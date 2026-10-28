//! AuditLoader's fixed column-unit adapter to the single typed metric parser.
//! Lexical checks here reject display text; no numeric conversion is duplicated.

use doctor_contracts::{
    AuditMetricFact, AuditMetricId, AuditMetricInput, AuditMetricState, ParseState,
};

use crate::{MetricDefinition, MetricDimension, MetricInput, ParseMode, parse_metric};

pub fn normalize_audit_metric(
    metric_id: AuditMetricId,
    input: AuditMetricInput,
) -> AuditMetricFact {
    let (dimension, suffix) = match metric_id {
        AuditMetricId::QueryTime | AuditMetricId::PendingTime => (MetricDimension::Duration, "ms"),
        AuditMetricId::CpuCost => (MetricDimension::Duration, "ns"),
        AuditMetricId::ScanBytes | AuditMetricId::MemoryCost => (MetricDimension::Bytes, "B"),
        AuditMetricId::ScanRows | AuditMetricId::ReturnRows => (MetricDimension::Count, ""),
    };
    let mut fact = AuditMetricFact {
        metric_id,
        input,
        state: AuditMetricState::Unavailable,
        parser_state: None,
        canonical_number: None,
        canonical_unit: None,
    };
    let parser_input = match &fact.input {
        AuditMetricInput::Unavailable { .. } => return fact,
        AuditMetricInput::SqlNull => MetricInput::Missing,
        AuditMetricInput::Text { value } => {
            // Source columns are integers, not human-formatted metric displays.
            if value.strip_prefix('-').is_some_and(|digits| {
                !digits.is_empty()
                    && digits.bytes().all(|byte| byte.is_ascii_digit())
                    && digits.bytes().any(|byte| byte != b'0')
            }) {
                fact.state = AuditMetricState::NegativeSentinel;
                return fact;
            }
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                fact.state = AuditMetricState::Invalid;
                return fact;
            }
            MetricInput::Text(format!("{value}{suffix}"))
        }
    };
    let mut definition = MetricDefinition::raw_exact("audit.fixed_column", dimension);
    definition.allow_unitless = dimension == MetricDimension::Count;
    let outcome = parse_metric(&definition, parser_input, ParseMode::Strict);
    fact.state = match outcome.parse_state {
        ParseState::Exact => AuditMetricState::Exact,
        ParseState::Missing => AuditMetricState::Missing,
        ParseState::Overflow => AuditMetricState::Overflow,
        ParseState::Invalid | ParseState::Partial => AuditMetricState::Invalid,
    };
    if let Some(number) = &outcome.canonical_number {
        // The wire value stays exact decimal text even when JS Number would round.
        const JS_MAX_EXACT_INTEGER: &str = "9007199254740991";
        if number.value.len() > JS_MAX_EXACT_INTEGER.len()
            || (number.value.len() == JS_MAX_EXACT_INTEGER.len()
                && number.value.as_str() > JS_MAX_EXACT_INTEGER)
        {
            fact.state = AuditMetricState::PrecisionUnsafe;
        }
    }
    fact.parser_state = Some(outcome.parse_state);
    fact.canonical_number = outcome.canonical_number;
    fact.canonical_unit = outcome.canonical_unit;
    fact
}
