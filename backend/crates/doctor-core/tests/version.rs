use doctor_contracts::{
    AnalysisReceipt, Compatibility, MergeState, MetricCellState, ProfileContext, ProfileLevel,
    QueryState, RequestedSourceKind, SnapshotKind, SupportStatus, VersionSource,
};
use doctor_core::{
    AdapterContext, AdapterLimits, GraphEvidence, InputEnvelope, VersionResolutionError,
    adapt_input, resolve_version_and_graph, v01_analysis_receipt,
};
use starrocks_version_pack_3_5_14::{PRODUCT_PACK_VERSION, SEMANTIC_MANIFEST_HASH};

fn receipt() -> AnalysisReceipt {
    v01_analysis_receipt()
}

fn profile(
    level: ProfileLevel,
    state: QueryState,
    snapshot: SnapshotKind,
    merge: MergeState,
) -> ProfileContext {
    ProfileContext {
        profile_level: level,
        query_state: state,
        snapshot_kind: snapshot,
        merge_state: merge,
        evidence_refs: vec!["json:".into()],
        limitations: vec![],
    }
}

fn adapter_output(content: &str, hint: Option<&str>) -> doctor_core::AdapterOutput {
    let adapter_context = AdapterContext::new(receipt()).expect("valid receipt");
    let mut input = InputEnvelope::new(RequestedSourceKind::JsonProfile, content);
    input.version_hint = hint.map(str::to_owned);
    adapt_input(input, &AdapterLimits::default(), &adapter_context).expect("adapter facts")
}

fn resolve(
    content: &str,
    hint: Option<&str>,
    context: ProfileContext,
) -> doctor_core::AdapterOutput {
    let mut output = adapter_output(content, hint);
    output.ir.profile_context = context;
    resolve_version_and_graph(
        &mut output,
        GraphEvidence::Unavailable {
            reason: "topology unavailable".into(),
            source_refs: vec!["json:".into()],
        },
    )
    .expect("resolve");
    output
}

#[test]
fn version_profile_goldens_resolve_without_pack_fallback() {
    let complete = profile(
        ProfileLevel::Level1,
        QueryState::Finished,
        SnapshotKind::Final,
        MergeState::Merged,
    );
    let detected = resolve(
        r#"{"profile_version":"3.5.14","operator_total_time_ns":"1"}"#,
        None,
        complete.clone(),
    );
    assert_eq!(detected.ir.version.version_source, VersionSource::Detected);
    assert_eq!(detected.ir.version.compatibility, Compatibility::Compatible);
    assert_eq!(detected.ir.version.support_status, SupportStatus::Supported);
    assert_eq!(
        detected
            .ir
            .version
            .selected_pack
            .as_ref()
            .map(|pack| pack.manifest_hash.as_str()),
        Some(SEMANTIC_MANIFEST_HASH)
    );

    let declared = resolve(
        r#"{"operator_total_time_ns":"1"}"#,
        Some("3.5.14"),
        complete.clone(),
    );
    assert_eq!(
        declared.ir.version.version_source,
        VersionSource::UserDeclared
    );
    assert_eq!(
        declared.ir.version.compatibility,
        Compatibility::Inconclusive
    );
    assert_eq!(declared.ir.version.support_status, SupportStatus::ParseOnly);
    assert!(declared.ir.version.selected_pack.is_some());
    assert_eq!(declared.facts.metric_cells[0].state, MetricCellState::Exact);
    assert!(
        declared
            .ir
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "metric_registry_candidate_parse_only")
    );
    assert!(
        declared
            .ir
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "version_user_declared_parse_only")
    );

    let unknown = resolve(r#"{"operator_total_time_ns":"1"}"#, None, complete.clone());
    assert_eq!(unknown.ir.version.version_source, VersionSource::Unknown);
    assert!(unknown.ir.version.selected_pack.is_none());
    assert_eq!(unknown.ir.version.support_status, SupportStatus::Unknown);
    assert_eq!(
        unknown.facts.metric_cells[0].state,
        MetricCellState::Unavailable
    );
    assert!(unknown.facts.metric_cells[0].canonical_number.is_none());
    assert_eq!(
        unknown.ir.metric_observations[0].semantic_state,
        "unavailable"
    );
    assert_eq!(unknown.ir.metric_observations[0].display_value, "—");
    assert_eq!(unknown.ir.metric_observations[0].strategy_origin, "unknown");
    assert_eq!(unknown.ir.completeness.metrics.present_count, 0);
    assert_eq!(
        unknown.ir.completeness.metrics.state,
        doctor_contracts::CompletenessState::Unavailable
    );
    assert!(
        unknown
            .ir
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "metric_registry_not_selected")
    );

    let other = resolve(
        r#"{"profile_version":"4.2.0","operator_total_time_ns":"1"}"#,
        Some("3.5.14"),
        complete.clone(),
    );
    assert!(other.ir.version.selected_pack.is_none());
    assert_eq!(other.ir.version.compatibility, Compatibility::Incompatible);
    assert_eq!(other.ir.version.support_status, SupportStatus::Unsupported);
    assert!(
        other
            .ir
            .version
            .compatibility_checks
            .iter()
            .any(|check| check.check_id == "version_hint_conflict")
    );
    assert_eq!(
        other.facts.metric_cells[0].state,
        MetricCellState::Incompatible
    );
    assert!(other.facts.metric_cells[0].canonical_number.is_none());
    assert_eq!(
        other.ir.metric_observations[0].semantic_state,
        "incompatible"
    );
    assert!(other.ir.metric_observations[0].definition_ref.is_none());
    assert_eq!(other.ir.completeness.metrics.present_count, 0);
    assert!(
        other
            .ir
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "version_incompatible")
    );

    for context in [
        profile(
            ProfileLevel::Level2,
            QueryState::Finished,
            SnapshotKind::Final,
            MergeState::Merged,
        ),
        profile(
            ProfileLevel::Level1,
            QueryState::Running,
            SnapshotKind::Runtime,
            MergeState::Unmerged,
        ),
        profile(
            ProfileLevel::Level1,
            QueryState::Finished,
            SnapshotKind::Final,
            MergeState::Unmerged,
        ),
    ] {
        let output = resolve(
            r#"{"profile_version":"3.5.14","operator_total_time_ns":"1"}"#,
            None,
            context,
        );
        assert_eq!(
            output.analysis_state,
            doctor_contracts::AnalysisState::Partial
        );
        assert_eq!(output.ir.version.compatibility, Compatibility::Partial);
        assert_eq!(output.ir.version.support_status, SupportStatus::ParseOnly);
        assert!(
            output
                .ir
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "profile_context_partial")
        );
    }
}

#[test]
fn resolver_rehashes_facts_but_keeps_the_source_receipt_replay_key() {
    let mut output = adapter_output(
        r#"{"profile_version":"3.5.14","operator_total_time_ns":"1"}"#,
        None,
    );
    output.ir.profile_context = profile(
        ProfileLevel::Level1,
        QueryState::Finished,
        SnapshotKind::Final,
        MergeState::Merged,
    );
    let key = output.ir.analysis_key.clone();
    let before = output.facts_hash();
    resolve_version_and_graph(
        &mut output,
        GraphEvidence::Unavailable {
            reason: "topology unavailable".into(),
            source_refs: vec!["json:".into()],
        },
    )
    .expect("resolve");
    assert_eq!(output.ir.analysis_key, key);
    assert_ne!(output.facts_hash(), before);
    assert!(
        output
            .ir
            .analysis_receipt
            .components
            .iter()
            .any(|component| {
                component.id == "starrocks-3.5.14"
                    && component.version == PRODUCT_PACK_VERSION
                    && component.manifest_hash == SEMANTIC_MANIFEST_HASH
            })
    );
}

#[test]
fn resolver_rejects_graph_receipt_hash_and_version_tampering() {
    for tamper_hash in [true, false] {
        let mut output = adapter_output(
            r#"{"profile_version":"3.5.14","operator_total_time_ns":"1"}"#,
            None,
        );
        let graph = output
            .ir
            .analysis_receipt
            .components
            .iter_mut()
            .find(|component| component.id == "sr3514-graph-resolver-contract")
            .expect("graph resolver receipt");
        if tamper_hash {
            graph.manifest_hash = "b".repeat(64);
        } else {
            graph.version = "0.0.0".into();
        }
        assert_eq!(
            resolve_version_and_graph(
                &mut output,
                GraphEvidence::Unavailable {
                    reason: "topology unavailable".into(),
                    source_refs: vec!["json:".into()],
                },
            ),
            Err(VersionResolutionError::ReceiptComponentInvalid(
                "graph_resolver"
            ))
        );
    }
}

#[test]
fn resolver_rejects_untrusted_receipt_and_non_frozen_hint() {
    let mut bad_receipt = receipt();
    bad_receipt
        .components
        .iter_mut()
        .find(|component| component.id == "starrocks-3.5.14")
        .expect("pack")
        .manifest_hash = "b".repeat(64);
    let context = AdapterContext::new(bad_receipt)
        .expect("structurally valid but semantically stale receipt");
    let mut output = adapt_input(
        InputEnvelope::new(
            RequestedSourceKind::JsonProfile,
            br#"{"profile_version":"3.5.14","operator_total_time_ns":"1"}"#.to_vec(),
        ),
        &AdapterLimits::default(),
        &context,
    )
    .expect("adapter facts");
    output.ir.profile_context = profile(
        ProfileLevel::Level1,
        QueryState::Finished,
        SnapshotKind::Final,
        MergeState::Merged,
    );
    assert_eq!(
        resolve_version_and_graph(
            &mut output,
            GraphEvidence::Unavailable {
                reason: "topology unavailable".into(),
                source_refs: vec!["json:".into()],
            }
        ),
        Err(VersionResolutionError::ReceiptComponentInvalid(
            "version_pack"
        ))
    );

    let mut invalid_hint = adapter_output(r#"{"operator_total_time_ns":"1"}"#, Some("4.2.0"));
    invalid_hint.ir.profile_context = profile(
        ProfileLevel::Level1,
        QueryState::Finished,
        SnapshotKind::Final,
        MergeState::Merged,
    );
    assert_eq!(
        resolve_version_and_graph(
            &mut invalid_hint,
            GraphEvidence::Unavailable {
                reason: "topology unavailable".into(),
                source_refs: vec!["json:".into()],
            }
        ),
        Err(VersionResolutionError::InvalidVersionHint)
    );

    let mut dangling_profile = adapter_output(
        r#"{"profile_version":"3.5.14","operator_total_time_ns":"1"}"#,
        None,
    );
    dangling_profile.ir.profile_context = ProfileContext {
        evidence_refs: vec!["json:/not-present".into()],
        ..profile(
            ProfileLevel::Level1,
            QueryState::Finished,
            SnapshotKind::Final,
            MergeState::Merged,
        )
    };
    assert_eq!(
        resolve_version_and_graph(
            &mut dangling_profile,
            GraphEvidence::Unavailable {
                reason: "topology unavailable".into(),
                source_refs: vec!["json:".into()],
            }
        ),
        Err(VersionResolutionError::GraphProvenanceInvalid)
    );

    let mut empty_profile = adapter_output(
        r#"{"profile_version":"3.5.14","operator_total_time_ns":"1"}"#,
        None,
    );
    empty_profile.ir.profile_context = ProfileContext {
        evidence_refs: vec![],
        ..profile(
            ProfileLevel::Level1,
            QueryState::Finished,
            SnapshotKind::Final,
            MergeState::Merged,
        )
    };
    assert_eq!(
        resolve_version_and_graph(
            &mut empty_profile,
            GraphEvidence::Unavailable {
                reason: "topology unavailable".into(),
                source_refs: vec!["json:".into()],
            }
        ),
        Err(VersionResolutionError::GraphProvenanceInvalid)
    );

    let mut dangling_graph = adapter_output(
        r#"{"profile_version":"3.5.14","operator_total_time_ns":"1"}"#,
        None,
    );
    dangling_graph.ir.profile_context = profile(
        ProfileLevel::Level1,
        QueryState::Finished,
        SnapshotKind::Final,
        MergeState::Merged,
    );
    assert_eq!(
        resolve_version_and_graph(
            &mut dangling_graph,
            GraphEvidence::Unavailable {
                reason: "topology unavailable".into(),
                source_refs: vec!["json:/not-present".into()],
            }
        ),
        Err(VersionResolutionError::GraphProvenanceInvalid)
    );
}

#[test]
fn profile_context_requires_full_evidence_before_supported() {
    let output = resolve(
        r#"{"profile_version":"3.5.14","operator_total_time_ns":"1"}"#,
        None,
        profile(
            ProfileLevel::Unknown,
            QueryState::Unknown,
            SnapshotKind::Unknown,
            MergeState::Unknown,
        ),
    );
    assert_eq!(output.ir.version.compatibility, Compatibility::Partial);
    assert_eq!(output.ir.version.support_status, SupportStatus::ParseOnly);
}

#[test]
fn version_fixture_cases_are_asserted_field_by_field() {
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_version_profile_contract_cases.json"
    ))
    .expect("frozen version cases");
    let find = |id| {
        cases
            .as_array()
            .expect("array")
            .iter()
            .find(|case| case["case_id"] == id)
            .expect("case")
    };
    let detected = find("version_detected_3514");
    assert_eq!(detected["version_source"], "detected");
    assert_eq!(detected["pack_match"], "exact_pack");
    assert_eq!(detected["compatibility"], "compatible");
    assert_eq!(detected["support_status"], "supported");
    let declared = find("version_user_declared_3514");
    assert_eq!(declared["version_source"], "user_declared");
    assert_eq!(declared["pack_match"], "exact_pack");
    assert_eq!(declared["compatibility"], "inconclusive");
    assert_eq!(declared["support_status"], "parse_only");
    let unknown = find("version_unknown");
    assert_eq!(unknown["pack_match"], "no_pack");
    assert_eq!(unknown["support_status"], "unknown");
    let other = find("version_known_other");
    assert_eq!(other["compatibility"], "incompatible");
    assert_eq!(other["support_status"], "unsupported");
    for id in ["profile_level_2", "profile_running", "profile_unmerged"] {
        assert_eq!(find(id)["analysis_state"], "partial");
        assert_eq!(find(id)["final_rule_status"], "not_applicable");
    }
}

/// Multi-version audit (tasks 26/27/34): an audited release from 3.1 on is the
/// 3.5.14 baseline plus its deltas; an unaudited patch, 2.5/3.0 or another line
/// never borrows a pack.
#[test]
fn audited_releases_resolve_to_the_baseline_with_their_deltas() {
    let complete = || {
        profile(
            ProfileLevel::Level1,
            QueryState::Finished,
            SnapshotKind::Final,
            MergeState::Merged,
        )
    };
    let delta = resolve(
        r#"{"profile_version":"3.3.5","operator_total_time_ns":"1"}"#,
        None,
        complete(),
    );
    let version = &delta.ir.version;
    assert_eq!(
        version.pack_match,
        doctor_contracts::PackMatch::BaselineWithDeltas
    );
    assert_eq!(version.compatibility, Compatibility::Compatible);
    assert_eq!(version.support_status, SupportStatus::Supported);
    let pack = version.selected_pack.as_ref().expect("baseline pack");
    assert_eq!(pack.upstream_release, "3.5.14");
    let deltas = pack.release_deltas.as_ref().expect("release deltas");
    assert_eq!(
        (deltas.release.as_str(), deltas.line.as_str()),
        ("3.3.5", "3.3")
    );
    assert!(
        deltas
            .delta_ids
            .iter()
            .any(|id| id == "merged_max_from_min_defect")
    );
    assert!(
        version
            .issues
            .iter()
            .any(|issue| issue == "merged_max_upstream_defect")
    );
    assert!(
        delta
            .ir
            .profile_context
            .limitations
            .iter()
            .any(|code| code == "merged_max_upstream_defect")
    );

    // 2.5 and 3.0 have no official ExplainAnalyzer: unsupported, never a nearby pack.
    for release in ["2.5.21", "3.0.9"] {
        let old = resolve(
            &format!(r#"{{"profile_version":"{release}","operator_total_time_ns":"1"}}"#),
            None,
            complete(),
        );
        assert!(old.ir.version.selected_pack.is_none(), "{release}");
        assert_eq!(
            old.ir.version.support_status,
            SupportStatus::Unsupported,
            "{release}"
        );
        assert_eq!(
            old.facts.metric_cells[0].state,
            MetricCellState::Incompatible,
            "{release}"
        );
    }

    let baseline = resolve(
        r#"{"profile_version":"3.5.14","operator_total_time_ns":"1"}"#,
        None,
        complete(),
    );
    assert_eq!(
        baseline.ir.version.pack_match,
        doctor_contracts::PackMatch::ExactPack
    );
    assert!(
        baseline
            .ir
            .version
            .selected_pack
            .as_ref()
            .is_some_and(|pack| pack.release_deltas.is_none())
    );

    let newer_patch = resolve(
        r#"{"profile_version":"4.1.8","operator_total_time_ns":"1"}"#,
        None,
        complete(),
    );
    assert!(newer_patch.ir.version.selected_pack.is_none());
    assert_eq!(
        newer_patch.ir.version.support_status,
        SupportStatus::Unsupported
    );
    assert!(
        newer_patch
            .ir
            .version
            .issues
            .iter()
            .any(|issue| issue == "release_not_audited")
    );
    assert_eq!(
        newer_patch.facts.metric_cells[0].state,
        MetricCellState::Incompatible
    );
}
