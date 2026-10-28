use doctor_contracts::{Fidelity, ParseState, SourceKind};
use doctor_core::{
    AnalysisAggregation, CounterStrategy, MetricDefinition, MetricDimension, MetricInput,
    MetricRegistryLookup, ParseMode, SourceCounterAggregate, SourceCounterMerge,
    SourceCounterMinMax, parse_metric, parse_source_counter_strategy, sr3514_metric_definition,
    sr3514_metric_lookup,
};

fn definition(case: &serde_json::Value) -> MetricDefinition {
    let dimension = match case["dimension"].as_str().unwrap() {
        "duration" => MetricDimension::Duration,
        "count" => MetricDimension::Count,
        "bytes" => MetricDimension::Bytes,
        "ratio" => MetricDimension::Ratio,
        other => panic!("unknown fixture dimension {other}"),
    };
    let id = case["case_id"].as_str().unwrap();
    if dimension == MetricDimension::Ratio {
        return if case["input"]
            .as_str()
            .is_some_and(|input| input.ends_with('%'))
        {
            MetricDefinition::bounded_percent(id)
        } else {
            MetricDefinition::raw_ratio(id)
        };
    }
    if matches!(case["fidelity"].as_str(), Some("rounded" | "truncated")) {
        MetricDefinition::sr3514_text_display(id, dimension)
    } else {
        let mut definition = MetricDefinition::raw_exact(id, dimension);
        definition.allow_unitless = case["source_kind"] == "json_profile";
        definition
    }
}

fn input(case: &serde_json::Value) -> MetricInput {
    match &case["input"] {
        serde_json::Value::Null => MetricInput::Missing,
        serde_json::Value::String(text) if case["source_kind"] == "json_profile" => {
            MetricInput::JsonNumber(text.clone())
        }
        serde_json::Value::String(text) => MetricInput::Text(text.clone()),
        other => panic!("unexpected fixture input {other}"),
    }
}

fn expected_state(value: &str) -> ParseState {
    match value {
        "exact" => ParseState::Exact,
        "missing" => ParseState::Missing,
        "invalid" => ParseState::Invalid,
        "partial" => ParseState::Partial,
        "overflow" => ParseState::Overflow,
        other => panic!("unknown expected state {other}"),
    }
}

#[test]
fn metrics_golden_cases_35_are_strict_and_lossless_about_state() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../../fixtures/expected/3.5.14/sr3514_metric_contract_cases.json"
    ))
    .unwrap();
    assert_eq!(
        cases.len(),
        35,
        "fixture case count changed; update the parser contract deliberately"
    );
    for case in &cases {
        let mode = if case["mode"] == "diagnostic_compat" {
            ParseMode::DiagnosticCompat
        } else {
            ParseMode::Strict
        };
        let outcome = parse_metric(&definition(case), input(case), mode);
        assert_eq!(
            outcome.parse_state,
            expected_state(case["parse_state"].as_str().unwrap()),
            "{}",
            case["case_id"]
        );
        assert_eq!(
            outcome
                .canonical_number
                .as_ref()
                .map(|number| number.value.as_str()),
            case["canonical_number"].as_str(),
            "{}",
            case["case_id"]
        );
        if outcome.parse_state == ParseState::Exact {
            assert!(outcome.unconsumed_ranges.is_empty(), "{}", case["case_id"]);
            assert_eq!(
                outcome.canonical_unit.as_deref(),
                Some(match case["dimension"].as_str().unwrap() {
                    "duration" => "ns",
                    "count" => "item",
                    "bytes" => "byte",
                    "ratio" => "ratio",
                    _ => unreachable!(),
                }),
                "{}",
                case["case_id"]
            );
            assert_eq!(
                outcome.fidelity,
                case["fidelity"].as_str().map(|value| match value {
                    "exact" => Fidelity::Exact,
                    "rounded" => Fidelity::Rounded,
                    "truncated" => Fidelity::Truncated,
                    _ => unreachable!(),
                }),
                "{}",
                case["case_id"]
            );
        } else {
            assert!(
                outcome.canonical_number.is_none() && outcome.bounds.is_none(),
                "{} must not acquire a fake zero",
                case["case_id"]
            );
        }
        if let Some(bounds) = case.get("bounds") {
            let actual = outcome
                .bounds
                .as_ref()
                .expect("fixture expects conservative bounds");
            assert_eq!(
                actual.lower.value,
                bounds["lower"].as_str().unwrap(),
                "{}",
                case["case_id"]
            );
            assert_eq!(
                actual.upper.value,
                bounds["upper"].as_str().unwrap(),
                "{}",
                case["case_id"]
            );
            assert_eq!(
                actual.lower_inclusive,
                bounds["lower_inclusive"].as_bool().unwrap(),
                "{}",
                case["case_id"]
            );
            assert_eq!(
                actual.upper_inclusive,
                bounds["upper_inclusive"].as_bool().unwrap(),
                "{}",
                case["case_id"]
            );
        }
        if let Some(code) = case["error_code"].as_str() {
            assert_eq!(
                outcome.error_code.map(|error| error.as_str()),
                Some(code),
                "{}",
                case["case_id"]
            );
        }
    }
    let strict = parse_metric(
        &MetricDefinition::raw_exact("strict", MetricDimension::Duration),
        MetricInput::Text("500msec".into()),
        ParseMode::Strict,
    );
    assert_eq!(strict.parse_state, ParseState::Invalid);
    assert!(strict.canonical_number.is_none());
    println!("metrics fixture cases checked: {}", cases.len());
}

#[test]
fn metrics_registry_keeps_source_counter_strategy_separate_from_product_aggregation() {
    let operator_total =
        sr3514_metric_definition(SourceKind::JsonProfile, "OperatorTotalTime").unwrap();
    let push_rows = sr3514_metric_definition(SourceKind::JsonProfile, "PushRowNum").unwrap();
    assert_eq!(
        operator_total.analysis_aggregation,
        AnalysisAggregation::NonAdditive
    );
    assert_eq!(
        push_rows.analysis_aggregation,
        AnalysisAggregation::NonAdditive
    );
    assert!(matches!(
        sr3514_metric_lookup(SourceKind::JsonProfile, "UnknownCounter"),
        MetricRegistryLookup::UnknownNonAdditiveUnavailable
    ));

    let source = parse_source_counter_strategy(&CounterStrategy {
        tunit: "TIME_NS".into(),
        aggregate_type: "sum".into(),
        merge_type: "skip_first_merge".into(),
        min_max_type: "min_max_all".into(),
        display_threshold: "0".into(),
    })
    .unwrap();
    assert_eq!(source.aggregate_type, SourceCounterAggregate::Sum);
    assert_eq!(source.merge_type, SourceCounterMerge::SkipFirstMerge);
    assert_eq!(source.min_max_type, SourceCounterMinMax::MinMaxAll);
}

#[test]
fn metrics_json_duration_requires_explicit_unitless_ns_policy() {
    let mut definition = MetricDefinition::raw_exact("json_ns", MetricDimension::Duration);
    definition.allow_unitless = true;

    let exact = parse_metric(
        &definition,
        MetricInput::JsonNumber("42".into()),
        ParseMode::Strict,
    );
    assert_eq!(exact.parse_state, ParseState::Exact);
    assert_eq!(exact.canonical_number.unwrap().value, "42");

    let non_integral = parse_metric(
        &definition,
        MetricInput::JsonNumber("0.1".into()),
        ParseMode::Strict,
    );
    assert_eq!(non_integral.parse_state, ParseState::Invalid);
    assert_eq!(
        non_integral.error_code.unwrap().as_str(),
        "non_integral_canonical"
    );

    let overflow = parse_metric(
        &definition,
        MetricInput::JsonNumber("9223372036854775808".into()),
        ParseMode::Strict,
    );
    assert_eq!(overflow.parse_state, ParseState::Overflow);
    assert_eq!(overflow.error_code.unwrap().as_str(), "numeric_overflow");

    let text = parse_metric(
        &definition,
        MetricInput::Text("42".into()),
        ParseMode::Strict,
    );
    assert_eq!(text.parse_state, ParseState::Invalid);
    assert_eq!(text.error_code.unwrap().as_str(), "unit_required");

    let spill_bytes = sr3514_metric_definition(SourceKind::JsonProfile, "QuerySpillBytes").unwrap();
    let text_bytes = parse_metric(
        &spill_bytes,
        MetricInput::Text("42".into()),
        ParseMode::Strict,
    );
    assert_eq!(text_bytes.error_code.unwrap().as_str(), "unit_required");
    let json_bytes = parse_metric(
        &spill_bytes,
        MetricInput::JsonNumber("42".into()),
        ParseMode::Strict,
    );
    assert_eq!(json_bytes.canonical_number.unwrap().value, "42");

    let push_rows = sr3514_metric_definition(SourceKind::TextProfile, "PushRowNum").unwrap();
    let text_count = parse_metric(
        &push_rows,
        MetricInput::Text("42".into()),
        ParseMode::Strict,
    );
    assert_eq!(text_count.canonical_number.unwrap().value, "42");
}

#[test]
fn metrics_registry_selects_source_aware_fidelity_policy() {
    let text_duration =
        sr3514_metric_definition(SourceKind::TextProfile, "OperatorTotalTime").unwrap();
    let duration = parse_metric(
        &text_duration,
        MetricInput::Text("500ms".into()),
        ParseMode::Strict,
    );
    assert_eq!(duration.parse_state, ParseState::Exact);
    assert_eq!(duration.fidelity, Some(Fidelity::Truncated));
    assert_eq!(duration.bounds.unwrap().upper.value, "500001000");

    let text_bytes = sr3514_metric_definition(SourceKind::TextProfile, "QuerySpillBytes").unwrap();
    let bytes = parse_metric(
        &text_bytes,
        MetricInput::Text("1.021 KB".into()),
        ParseMode::Strict,
    );
    assert_eq!(bytes.parse_state, ParseState::Exact);
    assert_eq!(bytes.fidelity, Some(Fidelity::Rounded));
    let bounds = bytes.bounds.unwrap();
    assert_eq!(bounds.lower.value, "1045");
    assert_eq!(bounds.upper.value, "1047");

    let json_bytes = sr3514_metric_definition(SourceKind::JsonProfile, "QuerySpillBytes").unwrap();
    let raw = parse_metric(
        &json_bytes,
        MetricInput::JsonNumber("1025".into()),
        ParseMode::Strict,
    );
    assert_eq!(raw.parse_state, ParseState::Exact);
    assert_eq!(raw.fidelity, Some(Fidelity::Exact));
    assert!(raw.bounds.is_none());
}

#[test]
fn metrics_invalid_literal_and_long_display_input_never_panic() {
    let bytes = MetricDefinition::sr3514_text_display("bytes", MetricDimension::Bytes);
    for input in [
        "null",
        "Infinity",
        "170141183460469231731687303715884105727.000 B",
    ] {
        let outcome = parse_metric(&bytes, MetricInput::Text(input.into()), ParseMode::Strict);
        let expected_state = if input == "null" || input == "Infinity" {
            ParseState::Invalid
        } else {
            ParseState::Overflow
        };
        assert_eq!(outcome.parse_state, expected_state, "{input}");
        assert_eq!(
            outcome.error_code.unwrap().as_str(),
            if input == "null" || input == "Infinity" {
                "invalid_number"
            } else {
                "numeric_overflow"
            },
            "{input}"
        );
    }
}

#[test]
fn metrics_display_intervals_must_contain_an_integer_counter() {
    let bytes = MetricDefinition::sr3514_text_display("bytes", MetricDimension::Bytes);
    let empty_bytes_interval = parse_metric(
        &bytes,
        MetricInput::Text("0.001 B".into()),
        ParseMode::Strict,
    );
    assert_eq!(empty_bytes_interval.parse_state, ParseState::Invalid);
    assert_eq!(
        empty_bytes_interval.error_code.unwrap().as_str(),
        "non_integral_canonical"
    );

    let count = MetricDefinition::sr3514_text_display("count", MetricDimension::Count);
    let lower_included = parse_metric(
        &count,
        MetricInput::Text("258.794M (258793500)".into()),
        ParseMode::Strict,
    );
    assert_eq!(lower_included.parse_state, ParseState::Exact);
    assert_eq!(lower_included.canonical_number.unwrap().value, "258793500");
    let upper_excluded = parse_metric(
        &count,
        MetricInput::Text("258.794M (258794500)".into()),
        ParseMode::Strict,
    );
    assert_eq!(upper_excluded.parse_state, ParseState::Invalid);
    assert_eq!(
        upper_excluded.error_code.unwrap().as_str(),
        "raw_count_mismatch"
    );
}

/// StarRocks 2.5.0–3.1.0 print counters differently (`printCounter` before
/// #28813); the same registry reads them with the legacy text format.
mod legacy_counter_text {
    use doctor_contracts::{Fidelity, ParseState, SourceKind};
    use doctor_core::{
        CounterTextFormat, InputPolicy, MetricInput, MetricRegistryLookup, ParseMode,
        metric_lookup_for, parse_metric,
    };

    fn parse(metric_id: &str, text: &str) -> doctor_core::MetricParseOutcome {
        let MetricRegistryLookup::Registered(definition) = metric_lookup_for(
            SourceKind::TextProfile,
            CounterTextFormat::LegacyUnpadded,
            metric_id,
        ) else {
            panic!("{metric_id} is registered");
        };
        assert_eq!(definition.input_policy, InputPolicy::LegacyTextDisplay);
        parse_metric(
            &definition,
            MetricInput::Text(text.into()),
            ParseMode::Strict,
        )
    }

    fn value(outcome: &doctor_core::MetricParseOutcome) -> String {
        outcome
            .canonical_number
            .as_ref()
            .expect("a number")
            .value
            .clone()
    }

    #[test]
    fn unpadded_fractions_are_the_remainder_of_the_next_unit() {
        // tmpValue / 1000 . tmpValue % 1000: 1_005_000 ns prints as "1.5ms".
        let ms = parse("OperatorTotalTime", "1.5ms");
        assert_eq!(ms.parse_state, ParseState::Exact);
        assert_eq!(value(&ms), "1005000");
        assert_eq!(ms.fidelity, Some(Fidelity::Truncated));
        let bounds = ms.bounds.expect("ns below a microsecond are truncated");
        assert_eq!(
            (bounds.lower.value.as_str(), bounds.upper.value.as_str()),
            ("1005000", "1006000")
        );
        assert_eq!(value(&parse("OperatorTotalTime", "1.500ms")), "1500000");
        assert_eq!(value(&parse("OperatorTotalTime", "12.34us")), "12034");
        assert_eq!(value(&parse("OperatorTotalTime", "3.0ms")), "3000000");
        assert_eq!(value(&parse("OperatorTotalTime", "1000ns")), "1000");
        assert_eq!(value(&parse("OperatorTotalTime", "1s234ms")), "1234000000");
    }

    #[test]
    fn legacy_grammar_rejects_what_the_old_printer_never_wrote() {
        for text in ["1.05ms", "1.00ms", "1.0005ms", "1.5s", "1.5ns"] {
            assert_eq!(
                parse("OperatorTotalTime", text).parse_state,
                ParseState::Invalid,
                "{text}"
            );
        }
    }

    #[test]
    fn legacy_bytes_have_two_decimals_and_zero_has_no_unit() {
        let zero = parse("BytesSent", "0.00");
        assert_eq!(
            (zero.parse_state.clone(), value(&zero)),
            (ParseState::Exact, "0".into())
        );
        let small = parse("BytesSent", "1024.00 B");
        assert_eq!(
            (value(&small), small.fidelity),
            ("1024".into(), Some(Fidelity::Exact))
        );
        let kib = parse("BytesSent", "1.50 KB");
        assert_eq!(kib.fidelity, Some(Fidelity::Rounded));
        let bounds = kib.bounds.expect("two-decimal rounding interval");
        assert_eq!(
            (bounds.lower.value.as_str(), bounds.upper.value.as_str()),
            ("1531", "1542")
        );
        assert_eq!(
            parse("BytesSent", "1.500 KB").parse_state,
            ParseState::Invalid
        );
    }

    #[test]
    fn legacy_counts_trust_the_bracketed_raw_value() {
        let count = parse("PushRowNum", "1.234567M (1234567)");
        assert_eq!(
            (count.parse_state.clone(), value(&count)),
            (ParseState::Exact, "1234567".into())
        );
        assert_eq!(value(&parse("PushRowNum", "999")), "999");
        assert_eq!(
            parse("PushRowNum", "1.2M (1234567)").parse_state,
            ParseState::Invalid
        );
        assert_eq!(
            parse("PushRowNum", "1.234567M").parse_state,
            ParseState::Invalid
        );
    }

    #[test]
    fn the_baseline_format_still_reads_a_padded_fraction_as_a_decimal() {
        let MetricRegistryLookup::Registered(definition) = metric_lookup_for(
            SourceKind::TextProfile,
            CounterTextFormat::Padded,
            "OperatorTotalTime",
        ) else {
            panic!("registered");
        };
        let outcome = parse_metric(
            &definition,
            MetricInput::Text("1.5ms".into()),
            ParseMode::Strict,
        );
        assert_eq!(value(&outcome), "1500000");
    }
}
