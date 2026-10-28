use doctor_contracts::{
    AuditFamilyDetailReadRequest, AuditFamilyRepresentative,
    AuditFamilyRepresentativeUnavailableReason,
};

#[test]
fn family_detail_accepts_only_opaque_bound_request_and_no_sql_or_bins() {
    let value = serde_json::json!({"connection_id":"c1", "discovery_generation":3,
        "scope":{"window":{"start_seconds":1,"end_seconds":60},"state":null,"user":null,
            "native_digest":null,"catalog":null,"database":null,"resource_group":null,"statement_filter":"all"},
        "family_key":format!("audit-family-v1:{}", "a".repeat(64))});
    assert!(serde_json::from_value::<AuditFamilyDetailReadRequest>(value.clone()).is_ok());
    for field in ["statement", "candidates", "bins", "native_digest", "limit"] {
        let mut extra = value.clone();
        extra[field] = serde_json::json!("client-controlled");
        assert!(serde_json::from_value::<AuditFamilyDetailReadRequest>(extra).is_err());
    }
}

#[test]
fn representative_unavailability_has_closed_distinct_reasons() {
    for (reason, wire) in [
        (
            AuditFamilyRepresentativeUnavailableReason::EmptyCohort,
            "empty_cohort",
        ),
        (
            AuditFamilyRepresentativeUnavailableReason::NoEligibleQueryTime,
            "no_eligible_query_time",
        ),
        (
            AuditFamilyRepresentativeUnavailableReason::NoMatchInBoundedProfileEvidence,
            "no_match_in_bounded_profile_evidence",
        ),
    ] {
        let json = serde_json::to_value(AuditFamilyRepresentative::Unavailable { reason }).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"state":"unavailable","reason":wire})
        );
    }
    assert!(
        serde_json::from_value::<AuditFamilyRepresentative>(
            serde_json::json!({"state":"unavailable","reason":"no_profile"})
        )
        .is_err()
    );
}
