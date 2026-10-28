use std::{ops::Range, sync::OnceLock};

use doctor_contracts::{
    BoundedInterval, CanonicalNumber, Fidelity, NumberKind, ParseState, SourceKind,
};

use crate::CounterStrategy;
pub use starrocks_version_pack_3_5_14::deltas::CounterTextFormat;

const I64_MAX: i128 = i64::MAX as i128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetricDimension {
    Duration,
    Count,
    Bytes,
    Ratio,
}

impl MetricDimension {
    fn canonical_unit(self) -> &'static str {
        match self {
            Self::Duration => "ns",
            Self::Count => "item",
            Self::Bytes => "byte",
            Self::Ratio => "ratio",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseMode {
    Strict,
    DiagnosticCompat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputPolicy {
    RawExact,
    Sr3514TextDisplay,
    /// Counters printed by StarRocks 2.5.0–3.1.0 (`printCounter` before #28813):
    /// ms/us fractions are the unpadded integer remainder of the next unit,
    /// bytes carry two decimals and a zero byte counter carries no unit.
    LegacyTextDisplay,
    DisplayCompat,
    RawRatio,
    BoundedPercent,
}

/// Product analysis choices. They are deliberately separate from StarRocks'
/// source-side `TCounterStrategy` and from VersionPack output-row recipes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnalysisAggregation {
    Sum,
    Max,
    Min,
    MinMaxRange,
    Weighted,
    NonAdditive,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MetricRegistryLookup {
    Registered(MetricDefinition),
    /// Frozen 3.5.14 default: unknown names are unavailable and cannot be aggregated.
    UnknownNonAdditiveUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetricDefinition {
    pub metric_id: String,
    pub dimension: MetricDimension,
    pub input_policy: InputPolicy,
    /// For duration/bytes this permits a raw JSON source number in the canonical unit;
    /// text still follows its unit grammar. Count also permits the official small-count text form.
    pub allow_unitless: bool,
    pub analysis_aggregation: AnalysisAggregation,
    pub valid_scopes: Vec<String>,
}

impl MetricDefinition {
    pub fn raw_exact(metric_id: impl Into<String>, dimension: MetricDimension) -> Self {
        Self {
            metric_id: metric_id.into(),
            dimension,
            input_policy: InputPolicy::RawExact,
            allow_unitless: matches!(dimension, MetricDimension::Ratio),
            analysis_aggregation: AnalysisAggregation::NonAdditive,
            valid_scopes: Vec::new(),
        }
    }

    pub fn sr3514_text_display(metric_id: impl Into<String>, dimension: MetricDimension) -> Self {
        let mut definition = Self::raw_exact(metric_id, dimension);
        definition.input_policy = InputPolicy::Sr3514TextDisplay;
        definition
    }

    pub fn raw_ratio(metric_id: impl Into<String>) -> Self {
        let mut definition = Self::raw_exact(metric_id, MetricDimension::Ratio);
        definition.input_policy = InputPolicy::RawRatio;
        definition
    }

    pub fn bounded_percent(metric_id: impl Into<String>) -> Self {
        let mut definition = Self::raw_exact(metric_id, MetricDimension::Ratio);
        definition.input_policy = InputPolicy::BoundedPercent;
        definition
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MetricInput {
    Missing,
    Text(String),
    JsonNumber(String),
}

impl MetricInput {
    fn text(&self) -> Option<&str> {
        match self {
            Self::Missing => None,
            Self::Text(text) | Self::JsonNumber(text) => Some(text),
        }
    }

    fn is_json_number(&self) -> bool {
        matches!(self, Self::JsonNumber(_))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetricParseError {
    MetricMissing,
    InvalidNumber,
    UnknownUnit,
    UnitRequired,
    TrailingInput,
    LeadingInput,
    DuplicateUnit,
    UnitOrder,
    DecimalNotLast,
    NonIntegralCanonical,
    RawCountMismatch,
    OutOfRange,
    NumericOverflow,
}

impl MetricParseError {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MetricMissing => "metric_missing",
            Self::InvalidNumber => "invalid_number",
            Self::UnknownUnit => "unknown_unit",
            Self::UnitRequired => "unit_required",
            Self::TrailingInput => "trailing_input",
            Self::LeadingInput => "leading_input",
            Self::DuplicateUnit => "duplicate_unit",
            Self::UnitOrder => "unit_order",
            Self::DecimalNotLast => "decimal_not_last",
            Self::NonIntegralCanonical => "non_integral_canonical",
            Self::RawCountMismatch => "raw_count_mismatch",
            Self::OutOfRange => "out_of_range",
            Self::NumericOverflow => "numeric_overflow",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetricParseOutcome {
    pub parse_state: ParseState,
    pub canonical_number: Option<CanonicalNumber>,
    pub canonical_unit: Option<String>,
    pub fidelity: Option<Fidelity>,
    pub bounds: Option<BoundedInterval>,
    pub consumed_ranges: Vec<Range<usize>>,
    pub unconsumed_ranges: Vec<Range<usize>>,
    pub error_code: Option<MetricParseError>,
    pub notes: Vec<String>,
}

impl MetricParseOutcome {
    fn missing() -> Self {
        Self {
            parse_state: ParseState::Missing,
            canonical_number: None,
            canonical_unit: None,
            fidelity: None,
            bounds: None,
            consumed_ranges: Vec::new(),
            unconsumed_ranges: Vec::new(),
            error_code: Some(MetricParseError::MetricMissing),
            notes: Vec::new(),
        }
    }

    fn exact(parsed: ParsedMetric, range: Range<usize>) -> Self {
        Self {
            parse_state: ParseState::Exact,
            canonical_number: Some(parsed.number),
            canonical_unit: Some(parsed.unit.to_owned()),
            fidelity: Some(parsed.fidelity),
            bounds: parsed.bounds,
            consumed_ranges: vec![range],
            unconsumed_ranges: Vec::new(),
            error_code: None,
            notes: Vec::new(),
        }
    }

    fn failure(error: Failure, mode: ParseMode, input_start: usize, input_end: usize) -> Self {
        let is_compat_partial = mode == ParseMode::DiagnosticCompat
            && error.code == MetricParseError::TrailingInput
            && error.consumed > 0;
        let consumed_end = input_start + error.consumed.min(input_end.saturating_sub(input_start));
        Self {
            parse_state: if is_compat_partial {
                ParseState::Partial
            } else if error.code == MetricParseError::NumericOverflow {
                ParseState::Overflow
            } else {
                ParseState::Invalid
            },
            canonical_number: None,
            canonical_unit: None,
            fidelity: None,
            bounds: None,
            consumed_ranges: (error.consumed > 0)
                .then_some(input_start..consumed_end)
                .into_iter()
                .collect(),
            unconsumed_ranges: (consumed_end < input_end)
                .then_some(consumed_end..input_end)
                .into_iter()
                .collect(),
            error_code: Some(error.code),
            notes: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceCounterAggregate {
    Sum,
    Avg,
    SumAvg,
    AvgSum,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceCounterMerge {
    MergeAll,
    SkipAll,
    SkipFirstMerge,
    SkipSecondMerge,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceCounterMinMax {
    MinMaxAll,
    SkipAll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceCounterStrategy {
    pub aggregate_type: SourceCounterAggregate,
    pub merge_type: SourceCounterMerge,
    pub min_max_type: SourceCounterMinMax,
}

/// This is the source merge stage from `TCounterStrategy`, never a product aggregation choice.
pub fn parse_source_counter_strategy(strategy: &CounterStrategy) -> Option<SourceCounterStrategy> {
    Some(SourceCounterStrategy {
        aggregate_type: match strategy.aggregate_type.as_str() {
            "sum" => SourceCounterAggregate::Sum,
            "avg" => SourceCounterAggregate::Avg,
            "sum_avg" => SourceCounterAggregate::SumAvg,
            "avg_sum" => SourceCounterAggregate::AvgSum,
            _ => return None,
        },
        merge_type: match strategy.merge_type.as_str() {
            "merge_all" => SourceCounterMerge::MergeAll,
            "skip_all" => SourceCounterMerge::SkipAll,
            "skip_first_merge" => SourceCounterMerge::SkipFirstMerge,
            "skip_second_merge" => SourceCounterMerge::SkipSecondMerge,
            _ => return None,
        },
        min_max_type: match strategy.min_max_type.as_str() {
            "min_max_all" => SourceCounterMinMax::MinMaxAll,
            "skip_all" => SourceCounterMinMax::SkipAll,
            _ => return None,
        },
    })
}

/// The 3.5.14 registry keeps source-counter merge semantics separate from product aggregation.
pub fn sr3514_metric_registry(source_kind: SourceKind) -> Vec<MetricDefinition> {
    metric_registry_for(source_kind, CounterTextFormat::Padded)
}

/// The baseline registry read with a release's counter text format. Legacy
/// counters are display strings in both JSON and text Profiles.
pub fn metric_registry_for(
    source_kind: SourceKind,
    format: CounterTextFormat,
) -> Vec<MetricDefinition> {
    let definition = |metric_id: &str, dimension, scopes: &[&str]| MetricDefinition {
        metric_id: metric_id.to_owned(),
        dimension,
        input_policy: match (&source_kind, format) {
            (_, CounterTextFormat::LegacyUnpadded) => InputPolicy::LegacyTextDisplay,
            (SourceKind::JsonProfile, CounterTextFormat::Padded) => InputPolicy::RawExact,
            (SourceKind::TextProfile, CounterTextFormat::Padded) => InputPolicy::Sr3514TextDisplay,
        },
        allow_unitless: match &source_kind {
            SourceKind::JsonProfile => true,
            SourceKind::TextProfile => dimension == MetricDimension::Count,
        },
        analysis_aggregation: AnalysisAggregation::NonAdditive,
        valid_scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
    };
    vec![
        definition(
            "QueryExecutionWallTime",
            MetricDimension::Duration,
            &["query"],
        ),
        // `RuntimeProfile::prettyPrint` emits Planner's direct `Total[1]`
        // counter independently from the Execution section.  The adapter
        // admits it only through that scoped alias; this registry entry does
        // not authorize matching arbitrary `Total` counters.
        definition("QueryPlanningTime", MetricDimension::Duration, &["query"]),
        // FE `Summary.Total`, admitted only from the Summary section.
        definition("QueryTotalTime", MetricDimension::Duration, &["query"]),
        definition(
            "QueryCumulativeCpuTime",
            MetricDimension::Duration,
            &["query"],
        ),
        definition(
            "QueryCumulativeOperatorTime",
            MetricDimension::Duration,
            &["query"],
        ),
        definition(
            "QueryCumulativeScanTime",
            MetricDimension::Duration,
            &["query"],
        ),
        definition(
            "QueryCumulativeNetworkTime",
            MetricDimension::Duration,
            &["query"],
        ),
        definition(
            "QueryPeakMemoryUsagePerNode",
            MetricDimension::Bytes,
            &["query"],
        ),
        definition(
            "QueryAllocatedMemoryUsage",
            MetricDimension::Bytes,
            &["query"],
        ),
        definition(
            "QueryDeallocatedMemoryUsage",
            MetricDimension::Bytes,
            &["query"],
        ),
        definition("QuerySumMemoryUsage", MetricDimension::Bytes, &["query"]),
        definition(
            "QueryPeakScheduleTime",
            MetricDimension::Duration,
            &["query"],
        ),
        definition("ResultDeliverTime", MetricDimension::Duration, &["query"]),
        definition(
            "FrontendProfileMergeTime",
            MetricDimension::Duration,
            &["query"],
        ),
        definition(
            "QuerySpillBytes",
            MetricDimension::Bytes,
            &["query", "operator"],
        ),
        definition(
            "Active",
            MetricDimension::Duration,
            &["profile", "fragment", "pipeline", "driver", "operator"],
        ),
        definition("ActiveTime", MetricDimension::Duration, &["pipeline"]),
        definition(
            "DriverTotalTime",
            MetricDimension::Duration,
            &["pipeline", "driver"],
        ),
        definition(
            "DriverPrepareTime",
            MetricDimension::Duration,
            &["pipeline", "driver"],
        ),
        definition(
            "PendingTime",
            MetricDimension::Duration,
            &["pipeline", "driver"],
        ),
        definition(
            "ScheduleTime",
            MetricDimension::Duration,
            &["pipeline", "driver"],
        ),
        definition(
            "ScheduleCount",
            MetricDimension::Count,
            &["pipeline", "driver"],
        ),
        definition(
            "OperatorTotalTime",
            MetricDimension::Duration,
            &["operator", "driver"],
        ),
        definition(
            "NetworkTime",
            MetricDimension::Duration,
            &["operator", "driver"],
        ),
        definition(
            "ScanTime",
            MetricDimension::Duration,
            &["operator", "driver"],
        ),
        definition("PushRowNum", MetricDimension::Count, &["operator", "sink"]),
        definition("PullRowNum", MetricDimension::Count, &["operator"]),
        definition(
            "PushChunkNum",
            MetricDimension::Count,
            &["operator", "sink"],
        ),
        definition("PullChunkNum", MetricDimension::Count, &["operator"]),
        definition(
            "PushTotalTime",
            MetricDimension::Duration,
            &["operator", "sink"],
        ),
        definition("PullTotalTime", MetricDimension::Duration, &["operator"]),
        definition(
            "OutputChunkBytes",
            MetricDimension::Bytes,
            &["operator", "sink"],
        ),
        definition("BytesSent", MetricDimension::Bytes, &["operator", "sink"]),
        definition(
            "InstanceNum",
            MetricDimension::Count,
            &["operator", "fragment"],
        ),
    ]
}

/// Query counters that StarRocks 3.5.14 sums over fragment-instance reports.
///
/// BE `GlobalDriverExecutor::report_exec_state` attaches the BE-wide
/// `QueryContext` value (`cpu_cost()` / `get_spill_bytes()`, shared by every
/// instance of the query on that BE) to each instance report, and FE
/// `QueryRuntimeProfile.buildQueryProfile` adds the value of every instance
/// of every fragment. The total therefore repeats one BE's counter once per
/// instance on that BE: it may double count, grows with the instance count,
/// and is not wall time. Each code is published on the query-scope cell and
/// keys Compare's parallelism gate.
const SR3514_INSTANCE_REPORT_SUM_METRICS: [(&str, &str); 3] = [
    (
        "QueryCumulativeCpuTime",
        "query_cpu_summed_over_instance_reports_not_wall_or_operator_time",
    ),
    (
        "QuerySpillBytes",
        "query_spill_summed_over_instance_reports",
    ),
    // Only 3.3.2–3.3.5 (version-deltas.json); the baseline takes the max per BE.
    (
        "QuerySumMemoryUsage",
        "query_memory_summed_over_instance_reports",
    ),
];

/// The limitation code of a query-scope metric that is a sum over instance
/// reports, or `None` for every other metric or scope.
pub fn sr3514_instance_report_sum_limitation(
    metric_id: &str,
    subject_ref: &str,
) -> Option<&'static str> {
    instance_report_sum_limitation(None, metric_id, subject_ref)
}

/// The same limitation for a reported release: the audited deltas add or
/// remove metrics (4.0.17 and 4.1.7 take the max per BE for CPU and spill).
/// Without a release, or for an unaudited one, the baseline list applies.
pub fn instance_report_sum_limitation(
    release: Option<&str>,
    metric_id: &str,
    subject_ref: &str,
) -> Option<&'static str> {
    if subject_ref != "query" {
        return None;
    }
    let summed = match release.map(starrocks_version_pack_3_5_14::deltas::resolve_release) {
        Some(starrocks_version_pack_3_5_14::deltas::ReleaseResolution::Audited(profile)) => {
            profile.instance_report_sum(metric_id)
        }
        _ => SR3514_INSTANCE_REPORT_SUM_METRICS[..2]
            .iter()
            .any(|(id, _)| *id == metric_id),
    };
    if !summed {
        return None;
    }
    SR3514_INSTANCE_REPORT_SUM_METRICS
        .iter()
        .find(|(id, _)| *id == metric_id)
        .map(|(_, code)| *code)
}

pub fn sr3514_metric_definition(
    source_kind: SourceKind,
    metric_id: &str,
) -> Option<MetricDefinition> {
    sr3514_registered_metric(source_kind, metric_id).cloned()
}

/// Borrowed registry entry for the per-counter hot paths.
pub(crate) fn sr3514_registered_metric(
    source_kind: SourceKind,
    metric_id: &str,
) -> Option<&'static MetricDefinition> {
    registered_metric_for(source_kind, CounterTextFormat::Padded, metric_id)
}

/// Borrowed registry entry for a source kind and counter text format.
pub(crate) fn registered_metric_for(
    source_kind: SourceKind,
    format: CounterTextFormat,
    metric_id: &str,
) -> Option<&'static MetricDefinition> {
    // The registry is a constant per source kind and format; lookups run once
    // per parsed counter, so each is built once instead of on every call.
    static JSON: OnceLock<Vec<MetricDefinition>> = OnceLock::new();
    static TEXT: OnceLock<Vec<MetricDefinition>> = OnceLock::new();
    static LEGACY_JSON: OnceLock<Vec<MetricDefinition>> = OnceLock::new();
    static LEGACY_TEXT: OnceLock<Vec<MetricDefinition>> = OnceLock::new();
    let registry = match (&source_kind, format) {
        (SourceKind::JsonProfile, CounterTextFormat::Padded) => &JSON,
        (SourceKind::TextProfile, CounterTextFormat::Padded) => &TEXT,
        (SourceKind::JsonProfile, CounterTextFormat::LegacyUnpadded) => &LEGACY_JSON,
        (SourceKind::TextProfile, CounterTextFormat::LegacyUnpadded) => &LEGACY_TEXT,
    }
    .get_or_init(|| metric_registry_for(source_kind.clone(), format));
    registry
        .iter()
        .find(|definition| definition.metric_id == metric_id)
}

pub fn sr3514_metric_lookup(source_kind: SourceKind, metric_id: &str) -> MetricRegistryLookup {
    metric_lookup_for(source_kind, CounterTextFormat::Padded, metric_id)
}

pub fn metric_lookup_for(
    source_kind: SourceKind,
    format: CounterTextFormat,
    metric_id: &str,
) -> MetricRegistryLookup {
    registered_metric_for(source_kind, format, metric_id)
        .cloned()
        .map(MetricRegistryLookup::Registered)
        .unwrap_or(MetricRegistryLookup::UnknownNonAdditiveUnavailable)
}

pub fn parse_metric(
    definition: &MetricDefinition,
    input: MetricInput,
    mode: ParseMode,
) -> MetricParseOutcome {
    let Some(raw) = input.text() else {
        return MetricParseOutcome::missing();
    };
    let is_json_number = input.is_json_number();
    let trimmed_start = raw.len() - raw.trim_start().len();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return MetricParseOutcome::missing();
    }
    let input_end = trimmed_start + trimmed.len();
    let parsed = match definition.dimension {
        MetricDimension::Duration => parse_duration(definition, trimmed, is_json_number),
        MetricDimension::Count => parse_count(definition, trimmed),
        MetricDimension::Bytes => parse_bytes(definition, trimmed, is_json_number),
        MetricDimension::Ratio => parse_ratio(definition, trimmed),
    };
    match parsed {
        Ok(metric) => MetricParseOutcome::exact(metric, trimmed_start..input_end),
        Err(failure) => MetricParseOutcome::failure(failure, mode, trimmed_start, input_end),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedMetric {
    number: CanonicalNumber,
    unit: &'static str,
    fidelity: Fidelity,
    bounds: Option<BoundedInterval>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Failure {
    code: MetricParseError,
    consumed: usize,
}

impl Failure {
    fn at(code: MetricParseError, consumed: usize) -> Self {
        Self { code, consumed }
    }
}

#[derive(Clone, Copy, Debug)]
struct Decimal {
    digits: i128,
    scale: u32,
    fractional: bool,
}

fn parse_duration(
    definition: &MetricDefinition,
    text: &str,
    is_json_number: bool,
) -> Result<ParsedMetric, Failure> {
    if !starts_number(text) {
        return Err(Failure::at(initial_error(text), 0));
    }
    if is_json_number && definition.allow_unitless {
        let (number, cursor) = parse_decimal(text, 0)?;
        if cursor != text.len() {
            return Err(Failure::at(MetricParseError::TrailingInput, cursor));
        }
        let value = decimal_to_i64(number, 1).map_err(|code| Failure::at(code, cursor))?;
        return Ok(ParsedMetric {
            number: integer(i128::from(value))?,
            unit: definition.dimension.canonical_unit(),
            fidelity: Fidelity::Exact,
            bounds: None,
        });
    }
    let mut cursor = 0;
    let mut total = 0_i128;
    let mut last_rank = 7_u8;
    let mut highest_rank = 0_u8;
    let mut seen = [false; 6];
    while cursor < text.len() {
        let (number, after_number) = parse_decimal(text, cursor)?;
        let Some((unit_index, unit_len, rank, multiplier)) = duration_unit(text, after_number)
        else {
            return Err(Failure::at(unit_error(text, after_number), after_number));
        };
        if seen[unit_index] {
            return Err(Failure::at(
                MetricParseError::DuplicateUnit,
                after_number + unit_len,
            ));
        }
        if rank >= last_rank {
            return Err(Failure::at(
                MetricParseError::UnitOrder,
                after_number + unit_len,
            ));
        }
        if number.fractional && after_number + unit_len < text.len() {
            return Err(Failure::at(
                MetricParseError::DecimalNotLast,
                after_number + unit_len,
            ));
        }
        let component =
            if definition.input_policy == InputPolicy::LegacyTextDisplay && number.fractional {
                legacy_duration_component(number, multiplier)
            } else {
                decimal_to_i64(number, multiplier)
            }
            .map_err(|code| Failure::at(code, after_number + unit_len))?;
        total = total.checked_add(i128::from(component)).ok_or_else(|| {
            Failure::at(MetricParseError::NumericOverflow, after_number + unit_len)
        })?;
        if total > I64_MAX {
            return Err(Failure::at(
                MetricParseError::NumericOverflow,
                after_number + unit_len,
            ));
        }
        seen[unit_index] = true;
        last_rank = rank;
        highest_rank = highest_rank.max(rank);
        cursor = after_number + unit_len;
        if cursor < text.len() && !text.as_bytes()[cursor].is_ascii_digit() {
            return Err(Failure::at(MetricParseError::TrailingInput, cursor));
        }
    }
    let canonical = integer(total)?;
    let width = match highest_rank {
        6 => 60_i128 * 1_000_000_000,
        5 => 1_000_000_000,
        4 => 1_000_000,
        3 => 1_000,
        _ => 0,
    };
    if matches!(
        definition.input_policy,
        InputPolicy::Sr3514TextDisplay | InputPolicy::LegacyTextDisplay
    ) && width > 0
    {
        let upper = total
            .checked_add(width)
            .ok_or_else(|| Failure::at(MetricParseError::NumericOverflow, text.len()))?;
        let (upper, upper_inclusive) = if upper > I64_MAX {
            (I64_MAX, true)
        } else {
            (upper, false)
        };
        return Ok(ParsedMetric {
            number: canonical,
            unit: definition.dimension.canonical_unit(),
            fidelity: Fidelity::Truncated,
            bounds: Some(integer_bounds(total, upper, upper_inclusive)?),
        });
    }
    Ok(ParsedMetric {
        number: canonical,
        unit: definition.dimension.canonical_unit(),
        fidelity: Fidelity::Exact,
        bounds: None,
    })
}

fn parse_count(definition: &MetricDefinition, text: &str) -> Result<ParsedMetric, Failure> {
    if !starts_number(text) {
        return Err(Failure::at(initial_error(text), 0));
    }
    let (number, mut cursor) = parse_decimal(text, 0)?;
    let (multiplier, has_unit) = match text.as_bytes().get(cursor).copied() {
        Some(b'K') => (1_000_i64, true),
        Some(b'M') => (1_000_000_i64, true),
        Some(b'B') => (1_000_000_000_i64, true),
        Some(byte) if byte.is_ascii_alphabetic() => {
            return Err(Failure::at(MetricParseError::UnknownUnit, cursor));
        }
        _ => (1_i64, false),
    };
    if has_unit {
        cursor += 1;
    }
    if !has_unit && !definition.allow_unitless {
        return Err(Failure::at(MetricParseError::UnitRequired, cursor));
    }
    let represented =
        decimal_to_i64(number, multiplier).map_err(|code| Failure::at(code, cursor))?;
    let raw_count = if text[cursor..].starts_with(" (") {
        let start = cursor + 2;
        let (raw_count, after_raw) = parse_integer(text, start)?;
        if text.as_bytes().get(after_raw) != Some(&b')') {
            return Err(Failure::at(MetricParseError::TrailingInput, after_raw));
        }
        cursor = after_raw + 1;
        Some(raw_count)
    } else {
        None
    };
    if cursor != text.len() {
        return Err(Failure::at(MetricParseError::TrailingInput, cursor));
    }
    if let Some(raw_count) = raw_count {
        let in_range = if matches!(
            definition.input_policy,
            InputPolicy::Sr3514TextDisplay | InputPolicy::DisplayCompat
        ) {
            display_bounds(number, multiplier, 3)
                .map_err(|code| Failure::at(code, text.len()))?
                .contains(raw_count)
        } else if definition.input_policy == InputPolicy::LegacyTextDisplay {
            // Legacy prints the quotient with Java's shortest round-trip
            // `Double.toString`; the bracketed raw count is authoritative.
            legacy_display_matches(number, multiplier, raw_count)
                .map_err(|code| Failure::at(code, text.len()))?
        } else {
            raw_count == represented
        };
        if !in_range {
            return Err(Failure::at(MetricParseError::RawCountMismatch, text.len()));
        }
        return Ok(ParsedMetric {
            number: integer(i128::from(raw_count))?,
            unit: definition.dimension.canonical_unit(),
            fidelity: Fidelity::Exact,
            bounds: None,
        });
    }
    if definition.input_policy == InputPolicy::LegacyTextDisplay && has_unit {
        // Legacy always prints the raw count next to a unit.
        return Err(Failure::at(MetricParseError::InvalidNumber, text.len()));
    }
    if matches!(
        definition.input_policy,
        InputPolicy::Sr3514TextDisplay | InputPolicy::DisplayCompat
    ) && has_unit
    {
        let display = decimal_times(number, multiplier)?;
        let bounds = display_bounds(number, multiplier, 3)
            .map_err(|code| Failure::at(code, text.len()))?
            .interval()?;
        return Ok(ParsedMetric {
            number: display,
            unit: definition.dimension.canonical_unit(),
            fidelity: Fidelity::Rounded,
            bounds: Some(bounds),
        });
    }
    Ok(ParsedMetric {
        number: integer(i128::from(represented))?,
        unit: definition.dimension.canonical_unit(),
        fidelity: Fidelity::Exact,
        bounds: None,
    })
}

fn parse_bytes(
    definition: &MetricDefinition,
    text: &str,
    is_json_number: bool,
) -> Result<ParsedMetric, Failure> {
    if !starts_number(text) {
        return Err(Failure::at(initial_error(text), 0));
    }
    let (number, after_number) = parse_decimal(text, 0)?;
    let mut cursor = after_number;
    if text[cursor..].starts_with(' ') {
        cursor += 1;
    }
    let Some((unit_len, multiplier)) = bytes_unit(text, cursor) else {
        // Legacy `getByteUint` gives zero no unit: "0.00 " prints as "0.00".
        if cursor == text.len()
            && definition.input_policy == InputPolicy::LegacyTextDisplay
            && !is_json_number
            && number.digits == 0
        {
            return Ok(ParsedMetric {
                number: integer(0)?,
                unit: definition.dimension.canonical_unit(),
                fidelity: Fidelity::Exact,
                bounds: None,
            });
        }
        if cursor == text.len() && definition.allow_unitless && is_json_number {
            let value = decimal_to_i64(number, 1).map_err(|code| Failure::at(code, cursor))?;
            return Ok(ParsedMetric {
                number: integer(i128::from(value))?,
                unit: definition.dimension.canonical_unit(),
                fidelity: Fidelity::Exact,
                bounds: None,
            });
        }
        return Err(Failure::at(
            if cursor == text.len() {
                MetricParseError::UnitRequired
            } else {
                MetricParseError::UnknownUnit
            },
            cursor,
        ));
    };
    cursor += unit_len;
    if cursor != text.len() {
        return Err(Failure::at(MetricParseError::TrailingInput, cursor));
    }
    if matches!(
        definition.input_policy,
        InputPolicy::Sr3514TextDisplay
            | InputPolicy::DisplayCompat
            | InputPolicy::LegacyTextDisplay
    ) {
        // The FE prints sub-KB byte counts (integers) as "%.3f B" (RuntimeProfile#printCounter,
        // DebugUtil#getByteUint): "0.000 B" / "512.000 B" are exact, not rounded.
        // Legacy releases print "%.2f B" and keep bytes up to 1024 in B.
        if multiplier == 1
            && let Ok(value) = decimal_to_i64(number, 1)
        {
            return Ok(ParsedMetric {
                number: integer(i128::from(value))?,
                unit: definition.dimension.canonical_unit(),
                fidelity: Fidelity::Exact,
                bounds: None,
            });
        }
        let display = decimal_times(number, multiplier)?;
        let scale = if definition.input_policy == InputPolicy::LegacyTextDisplay {
            2
        } else {
            3
        };
        let bounds = display_bounds(number, multiplier, scale)
            .map_err(|code| Failure::at(code, text.len()))?
            .interval()?;
        return Ok(ParsedMetric {
            number: display,
            unit: definition.dimension.canonical_unit(),
            fidelity: Fidelity::Rounded,
            bounds: Some(bounds),
        });
    }
    let value = decimal_to_i64(number, multiplier).map_err(|code| Failure::at(code, text.len()))?;
    Ok(ParsedMetric {
        number: integer(i128::from(value))?,
        unit: definition.dimension.canonical_unit(),
        fidelity: Fidelity::Exact,
        bounds: None,
    })
}

fn parse_ratio(definition: &MetricDefinition, text: &str) -> Result<ParsedMetric, Failure> {
    if !starts_number(text) {
        return Err(Failure::at(initial_error(text), 0));
    }
    let (mut number, mut cursor) = parse_decimal(text, 0)?;
    let percent = text.as_bytes().get(cursor) == Some(&b'%');
    if percent {
        cursor += 1;
    }
    if cursor != text.len() {
        return Err(Failure::at(MetricParseError::TrailingInput, cursor));
    }
    match (definition.input_policy, percent) {
        (InputPolicy::BoundedPercent, true) => {
            if decimal_greater_than(number, 100)? {
                return Err(Failure::at(MetricParseError::OutOfRange, text.len()));
            }
            number.scale = number
                .scale
                .checked_add(2)
                .ok_or_else(|| Failure::at(MetricParseError::NumericOverflow, text.len()))?;
        }
        (InputPolicy::RawRatio, false) => {}
        (_, true) => return Err(Failure::at(MetricParseError::UnknownUnit, cursor - 1)),
        _ => return Err(Failure::at(MetricParseError::UnitRequired, cursor)),
    }
    if number.scale > 18 {
        return Err(Failure::at(MetricParseError::NumericOverflow, text.len()));
    }
    let canonical = canonical_decimal(number.digits, number.scale)
        .map_err(|code| Failure::at(code, text.len()))?;
    Ok(ParsedMetric {
        number: canonical,
        unit: definition.dimension.canonical_unit(),
        fidelity: Fidelity::Exact,
        bounds: None,
    })
}

fn starts_number(text: &str) -> bool {
    text.as_bytes().first().is_some_and(u8::is_ascii_digit)
}

fn initial_error(text: &str) -> MetricParseError {
    if text == "null"
        || text.starts_with("NaN")
        || text.starts_with("Infinity")
        || matches!(text.as_bytes().first(), Some(b'+' | b'-' | b'.'))
    {
        MetricParseError::InvalidNumber
    } else {
        MetricParseError::LeadingInput
    }
}

fn unit_error(text: &str, cursor: usize) -> MetricParseError {
    match text.as_bytes().get(cursor).copied() {
        None => MetricParseError::UnitRequired,
        Some(b'e' | b'E' | b'.' | b'+' | b'-') => MetricParseError::InvalidNumber,
        Some(_) => MetricParseError::UnknownUnit,
    }
}

fn parse_decimal(text: &str, start: usize) -> Result<(Decimal, usize), Failure> {
    let bytes = text.as_bytes();
    let mut cursor = start;
    if !bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
        return Err(Failure::at(MetricParseError::InvalidNumber, cursor));
    }
    let integer_start = cursor;
    while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
        cursor += 1;
    }
    let mut fractional = false;
    let fraction_start = if bytes.get(cursor) == Some(&b'.') {
        fractional = true;
        cursor += 1;
        let start = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if start == cursor {
            return Err(Failure::at(MetricParseError::InvalidNumber, cursor));
        }
        start
    } else {
        cursor
    };
    let mut digits = 0_i128;
    for byte in bytes[integer_start..cursor]
        .iter()
        .filter(|byte| **byte != b'.')
    {
        digits = digits
            .checked_mul(10)
            .and_then(|value| value.checked_add(i128::from(*byte - b'0')))
            .ok_or_else(|| Failure::at(MetricParseError::NumericOverflow, cursor))?;
    }
    let scale = u32::try_from(cursor - fraction_start)
        .map_err(|_| Failure::at(MetricParseError::NumericOverflow, cursor))?;
    Ok((
        Decimal {
            digits,
            scale,
            fractional,
        },
        cursor,
    ))
}

fn parse_integer(text: &str, start: usize) -> Result<(i64, usize), Failure> {
    let bytes = text.as_bytes();
    let mut cursor = start;
    if !bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
        return Err(Failure::at(MetricParseError::InvalidNumber, cursor));
    }
    let mut value = 0_i128;
    while let Some(byte) = bytes.get(cursor).copied().filter(u8::is_ascii_digit) {
        value = value
            .checked_mul(10)
            .and_then(|current| current.checked_add(i128::from(byte - b'0')))
            .ok_or_else(|| Failure::at(MetricParseError::NumericOverflow, cursor))?;
        if value > I64_MAX {
            return Err(Failure::at(MetricParseError::NumericOverflow, cursor));
        }
        cursor += 1;
    }
    Ok((value as i64, cursor))
}

fn duration_unit(text: &str, cursor: usize) -> Option<(usize, usize, u8, i64)> {
    let suffix = &text[cursor..];
    [
        ("ms", 3, 1_000_000_i64),
        ("us", 2, 1_000_i64),
        ("ns", 1, 1_i64),
        ("h", 6, 3_600_000_000_000_i64),
        ("m", 5, 60_000_000_000_i64),
        ("s", 4, 1_000_000_000_i64),
    ]
    .into_iter()
    .enumerate()
    .find_map(|(index, (unit, rank, multiplier))| {
        suffix
            .starts_with(unit)
            .then_some((index, unit.len(), rank, multiplier))
    })
}

fn bytes_unit(text: &str, cursor: usize) -> Option<(usize, i64)> {
    let suffix = &text[cursor..];
    [
        ("KB", 1_024_i64),
        ("MB", 1_048_576_i64),
        ("GB", 1_073_741_824_i64),
        ("TB", 1_099_511_627_776_i64),
        ("B", 1_i64),
    ]
    .into_iter()
    .find_map(|(unit, multiplier)| suffix.starts_with(unit).then_some((unit.len(), multiplier)))
}

fn decimal_to_i64(number: Decimal, multiplier: i64) -> Result<i64, MetricParseError> {
    let product = number
        .digits
        .checked_mul(i128::from(multiplier))
        .ok_or(MetricParseError::NumericOverflow)?;
    let divisor = pow10(number.scale)?;
    if product % divisor != 0 {
        return Err(MetricParseError::NonIntegralCanonical);
    }
    let value = product / divisor;
    if value > I64_MAX {
        return Err(MetricParseError::NumericOverflow);
    }
    Ok(value as i64)
}

fn decimal_times(number: Decimal, multiplier: i64) -> Result<CanonicalNumber, Failure> {
    let product = number
        .digits
        .checked_mul(i128::from(multiplier))
        .ok_or_else(|| Failure::at(MetricParseError::NumericOverflow, 0))?;
    let divisor = pow10(number.scale).map_err(|code| Failure::at(code, 0))?;
    if product / divisor > I64_MAX {
        return Err(Failure::at(MetricParseError::NumericOverflow, 0));
    }
    canonical_decimal(product, number.scale).map_err(|code| Failure::at(code, 0))
}

fn integer(value: i128) -> Result<CanonicalNumber, Failure> {
    if !(0..=I64_MAX).contains(&value) {
        return Err(Failure::at(MetricParseError::NumericOverflow, 0));
    }
    Ok(CanonicalNumber {
        kind: NumberKind::Integer,
        value: value.to_string(),
        scale: None,
    })
}

fn canonical_decimal(
    mut digits: i128,
    mut scale: u32,
) -> Result<CanonicalNumber, MetricParseError> {
    let divisor = pow10(scale)?;
    if digits / divisor > I64_MAX {
        return Err(MetricParseError::NumericOverflow);
    }
    while scale > 0 && digits % 10 == 0 {
        digits /= 10;
        scale -= 1;
    }
    if scale == 0 {
        return Ok(CanonicalNumber {
            kind: NumberKind::Integer,
            value: digits.to_string(),
            scale: None,
        });
    }
    let raw = digits.to_string();
    let scale_usize = scale as usize;
    let value = if raw.len() <= scale_usize {
        format!("0.{}{}", "0".repeat(scale_usize - raw.len()), raw)
    } else {
        let split = raw.len() - scale_usize;
        format!("{}.{}", &raw[..split], &raw[split..])
    };
    Ok(CanonicalNumber {
        kind: NumberKind::Decimal,
        value,
        scale: Some(scale),
    })
}

fn pow10(scale: u32) -> Result<i128, MetricParseError> {
    (0..scale).try_fold(1_i128, |value, _| {
        value
            .checked_mul(10)
            .ok_or(MetricParseError::NumericOverflow)
    })
}

fn decimal_greater_than(number: Decimal, integer: i64) -> Result<bool, Failure> {
    let scaled = i128::from(integer)
        .checked_mul(pow10(number.scale).map_err(|code| Failure::at(code, 0))?)
        .ok_or_else(|| Failure::at(MetricParseError::NumericOverflow, 0))?;
    Ok(number.digits > scaled)
}

struct DisplayBounds {
    lower: i64,
    upper: i64,
    upper_inclusive: bool,
}

impl DisplayBounds {
    fn contains(&self, value: i64) -> bool {
        value >= self.lower && (value < self.upper || (self.upper_inclusive && value == self.upper))
    }

    fn interval(&self) -> Result<BoundedInterval, Failure> {
        integer_bounds(
            i128::from(self.lower),
            i128::from(self.upper),
            self.upper_inclusive,
        )
    }
}

/// Legacy "A.Bms" / "A.Bus": B is the unpadded integer remainder of the next
/// unit (1–3 digits, no leading zero), so "1.5ms" is 1 ms + 5 us.
fn legacy_duration_component(number: Decimal, multiplier: i64) -> Result<i64, MetricParseError> {
    if !matches!(multiplier, 1_000_000 | 1_000) || !(1..=3).contains(&number.scale) {
        return Err(MetricParseError::InvalidNumber);
    }
    let divisor = pow10(number.scale)?;
    let (whole, remainder) = (number.digits / divisor, number.digits % divisor);
    if number.scale > 1 && remainder < divisor / 10 {
        return Err(MetricParseError::InvalidNumber);
    }
    let value = whole
        .checked_mul(i128::from(multiplier))
        .and_then(|value| value.checked_add(remainder * i128::from(multiplier / 1_000)))
        .ok_or(MetricParseError::NumericOverflow)?;
    if value > I64_MAX {
        return Err(MetricParseError::NumericOverflow);
    }
    Ok(value as i64)
}

/// Whether a legacy display quotient agrees with its raw count to within one unit.
fn legacy_display_matches(
    number: Decimal,
    multiplier: i64,
    raw: i64,
) -> Result<bool, MetricParseError> {
    let shown = number
        .digits
        .checked_mul(i128::from(multiplier))
        .ok_or(MetricParseError::NumericOverflow)?;
    let raw_scaled = i128::from(raw)
        .checked_mul(pow10(number.scale)?)
        .ok_or(MetricParseError::NumericOverflow)?;
    Ok((shown - raw_scaled).abs() < pow10(number.scale)?)
}

fn display_bounds(
    number: Decimal,
    multiplier: i64,
    scale: u32,
) -> Result<DisplayBounds, MetricParseError> {
    if number.scale != scale {
        return Err(MetricParseError::InvalidNumber);
    }
    let denominator = 2_i128
        .checked_mul(pow10(number.scale)?)
        .ok_or(MetricParseError::NumericOverflow)?;
    let lower_numerator = number
        .digits
        .checked_mul(2)
        .and_then(|value| value.checked_sub(1))
        .ok_or(MetricParseError::NumericOverflow)?;
    let upper_numerator = number
        .digits
        .checked_mul(2)
        .and_then(|value| value.checked_add(1))
        .ok_or(MetricParseError::NumericOverflow)?;
    let lower = if lower_numerator <= 0 {
        0
    } else {
        ceil_div(
            lower_numerator
                .checked_mul(i128::from(multiplier))
                .ok_or(MetricParseError::NumericOverflow)?,
            denominator,
        )?
    };
    let upper = ceil_div(
        upper_numerator
            .checked_mul(i128::from(multiplier))
            .ok_or(MetricParseError::NumericOverflow)?,
        denominator,
    )?;
    if lower > I64_MAX {
        return Err(MetricParseError::NumericOverflow);
    }
    if upper <= lower {
        return Err(MetricParseError::NonIntegralCanonical);
    }
    Ok(if upper > I64_MAX {
        DisplayBounds {
            lower: lower as i64,
            upper: i64::MAX,
            upper_inclusive: true,
        }
    } else {
        DisplayBounds {
            lower: lower as i64,
            upper: upper as i64,
            upper_inclusive: false,
        }
    })
}

fn ceil_div(value: i128, divisor: i128) -> Result<i128, MetricParseError> {
    let quotient = value / divisor;
    if value % divisor == 0 {
        Ok(quotient)
    } else {
        quotient
            .checked_add(1)
            .ok_or(MetricParseError::NumericOverflow)
    }
}

fn integer_bounds(
    lower: i128,
    upper: i128,
    upper_inclusive: bool,
) -> Result<BoundedInterval, Failure> {
    Ok(BoundedInterval {
        lower: integer(lower)?,
        upper: integer(upper)?,
        lower_inclusive: true,
        upper_inclusive,
    })
}
