use doctor_contracts::{
    AuditMetricId, AuditMetricInput, AuditMetricState, AuditOptionalFactUnavailableReason,
    ParseState,
};
use doctor_core::normalize_audit_metric;

fn text(value: &str) -> AuditMetricInput {
    AuditMetricInput::Text {
        value: value.into(),
    }
}

#[test]
fn audit_null_empty_zero_negative_and_invalid_remain_distinct() {
    for (input, expected) in [
        (AuditMetricInput::SqlNull, AuditMetricState::Missing),
        (text(""), AuditMetricState::Invalid),
        (text("0"), AuditMetricState::Exact),
        (text("-1"), AuditMetricState::NegativeSentinel),
        (
            text("-9223372036854775808"),
            AuditMetricState::NegativeSentinel,
        ),
        (text("-0"), AuditMetricState::Invalid),
        (text(" 0"), AuditMetricState::Invalid),
        (text("1ms"), AuditMetricState::Invalid),
        (text("1.5"), AuditMetricState::Invalid),
        (text("1e3"), AuditMetricState::Invalid),
        (text("bad"), AuditMetricState::Invalid),
    ] {
        let fact = normalize_audit_metric(AuditMetricId::QueryTime, input.clone());
        assert_eq!(fact.state, expected, "{input:?}");
        assert_eq!(fact.input, input);
        assert!(fact.validate());
        assert_eq!(
            fact.canonical_number.is_some(),
            expected == AuditMetricState::Exact
        );
    }
}

#[test]
fn audit_column_units_are_fixed_and_use_the_shared_parser() {
    for (id, input, value, unit) in [
        (AuditMetricId::QueryTime, "125", "125000000", "ns"),
        (AuditMetricId::PendingTime, "125", "125000000", "ns"),
        (AuditMetricId::CpuCost, "125", "125", "ns"),
        (AuditMetricId::ScanBytes, "1024", "1024", "byte"),
        (AuditMetricId::MemoryCost, "0", "0", "byte"),
        (AuditMetricId::ScanRows, "42", "42", "item"),
        (AuditMetricId::ReturnRows, "0001", "1", "item"),
    ] {
        let fact = normalize_audit_metric(id, text(input));
        assert_eq!(fact.state, AuditMetricState::Exact);
        assert_eq!(fact.parser_state, Some(ParseState::Exact));
        assert_eq!(fact.canonical_number.unwrap().value, value);
        assert_eq!(fact.canonical_unit.as_deref(), Some(unit));
    }
}

#[test]
fn pending_time_uses_the_same_millisecond_parser_and_typed_boundaries() {
    for input in [
        AuditMetricInput::SqlNull,
        text(""),
        text("0"),
        text("-1"),
        text("-0"),
        text("1.5"),
        text("1ms"),
        text("9007199255"),
        text("9223372036854"),
        text("9223372036855"),
    ] {
        let expected = normalize_audit_metric(AuditMetricId::QueryTime, input.clone());
        let mut pending = normalize_audit_metric(AuditMetricId::PendingTime, input);
        assert_eq!(pending.metric_id, AuditMetricId::PendingTime);
        pending.metric_id = AuditMetricId::QueryTime;
        assert_eq!(pending, expected);
    }
}

#[test]
fn absent_and_conflicting_optional_facts_never_enter_the_numeric_parser() {
    for reason in [
        AuditOptionalFactUnavailableReason::SourceColumnAbsent,
        AuditOptionalFactUnavailableReason::ConflictingSourceRows,
    ] {
        for id in [
            AuditMetricId::QueryTime,
            AuditMetricId::PendingTime,
            AuditMetricId::CpuCost,
            AuditMetricId::ScanBytes,
            AuditMetricId::ScanRows,
            AuditMetricId::MemoryCost,
            AuditMetricId::ReturnRows,
        ] {
            let input = AuditMetricInput::Unavailable {
                reason: reason.clone(),
            };
            let fact = normalize_audit_metric(id, input.clone());
            assert_eq!(fact.input, input);
            assert_eq!(fact.state, AuditMetricState::Unavailable);
            assert!(fact.parser_state.is_none());
            assert!(fact.canonical_number.is_none());
            assert!(fact.canonical_unit.is_none());
            assert!(fact.validate());
        }
    }
}

#[test]
fn audit_precision_risk_keeps_exact_text_and_overflow_keeps_no_fake_value() {
    for (input, state) in [
        ("9007199254740991", AuditMetricState::Exact),
        ("9007199254740992", AuditMetricState::PrecisionUnsafe),
        ("9223372036854775807", AuditMetricState::PrecisionUnsafe),
        ("9223372036854775808", AuditMetricState::Overflow),
    ] {
        let fact = normalize_audit_metric(AuditMetricId::ScanRows, text(input));
        assert_eq!(fact.state, state);
        assert!(fact.validate());
        if state == AuditMetricState::Overflow {
            assert!(fact.canonical_number.is_none());
        } else {
            assert_eq!(fact.canonical_number.unwrap().value, input);
        }
    }
    let scaled = normalize_audit_metric(AuditMetricId::QueryTime, text("9223372036855"));
    assert_eq!(scaled.state, AuditMetricState::Overflow);
    assert_eq!(scaled.parser_state, Some(ParseState::Overflow));
    assert!(scaled.canonical_number.is_none());
}
