use doctor_contracts::{
    AuditErrorCodeFact, AuditErrorProportion, AuditErrorProportionInvalidReason,
    AuditErrorProportionUnavailableReason, AuditFamilyPresentation, AuditFamilySort,
    AuditMetricInput, AuditOptionalFactUnavailableReason,
};
use schemars::schema_for;
use serde_json::json;
use ts_rs::TS;

#[test]
fn audit_family_error_proportion_sort_has_a_stable_wire_value() {
    assert_eq!(
        serde_json::to_value(AuditFamilySort::ErrorProportionDesc).expect("family sort serializes"),
        json!("error_proportion_desc")
    );
    assert!(AuditFamilySort::decl().contains("error_proportion_desc"));
}

#[test]
fn audit_error_proportion_is_typed_and_old_family_payloads_default_to_unavailable() {
    let old: AuditFamilyPresentation = serde_json::from_value(json!({
        "membership": {
            "state": "exact_native_digest",
            "family_key": "family-key",
            "native_digest": "a0b1",
            "context": {"cluster": "session:a", "engine_version": "3.5.14", "catalog": "default_catalog", "database": "demo"}
        },
        "execution_count": "3",
        "error_count": "1",
        "aggregates": []
    }))
    .expect("old family payload remains readable");
    assert_eq!(
        old.error_proportion,
        AuditErrorProportion::Unavailable {
            reason: AuditErrorProportionUnavailableReason::LegacyFieldAbsent,
        }
    );

    let schema =
        serde_json::to_value(schema_for!(AuditErrorProportion)).expect("proportion schema");
    for state in ["exact", "not_applicable", "unavailable", "invalid"] {
        assert!(
            schema.to_string().contains(state),
            "missing proportion arm {state}"
        );
    }
    let declaration = AuditErrorProportion::decl();
    assert!(declaration.contains("numerator: string"));
    assert!(declaration.contains("denominator: string"));
    assert!(declaration.contains("reason: AuditErrorProportionInvalidReason"));
    assert_eq!(
        serde_json::to_value(AuditErrorProportion::Invalid {
            reason: AuditErrorProportionInvalidReason::NumeratorExceedsDenominator,
        })
        .expect("invalid proportion serializes"),
        json!({"state": "invalid", "reason": "numerator_exceeds_denominator"})
    );
}

#[test]
fn optional_audit_facts_keep_unavailable_null_and_empty_text_distinct() {
    let absent = AuditMetricInput::Unavailable {
        reason: AuditOptionalFactUnavailableReason::SourceColumnAbsent,
    };
    assert_eq!(
        serde_json::to_value(absent).expect("unavailable metric input serializes"),
        json!({"state": "unavailable", "reason": "source_column_absent"})
    );

    for (fact, expected) in [
        (
            AuditErrorCodeFact::Unavailable {
                reason: AuditOptionalFactUnavailableReason::ConflictingSourceRows,
            },
            json!({"state": "unavailable", "reason": "conflicting_source_rows"}),
        ),
        (AuditErrorCodeFact::SqlNull, json!({"state": "sql_null"})),
        (
            AuditErrorCodeFact::Text {
                value: String::new(),
            },
            json!({"state": "text", "value": ""}),
        ),
    ] {
        assert_eq!(
            serde_json::to_value(fact).expect("fact serializes"),
            expected
        );
    }

    assert_eq!(
        AuditErrorCodeFact::default(),
        AuditErrorCodeFact::Unavailable {
            reason: AuditOptionalFactUnavailableReason::SourceColumnAbsent,
        }
    );
}
