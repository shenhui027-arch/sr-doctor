//! One rule set for StarRocks 3.1 and later (3.1–4.1 audited): the 3.5.14
//! baseline plus the release deltas in `version-deltas.json`, found by the
//! multi-version source audit (`scripts/version-audit/`). A release outside
//! every audited range never borrows a nearby line. 2.5 and 3.0 are not
//! supported: they ship no `ExplainAnalyzer`, so their time share, execution
//! graph and Profile state would all be product guesses.

use std::sync::OnceLock;

use doctor_contracts::canonical_hash;
use serde::Deserialize;

use crate::{UPSTREAM_COMMIT, UPSTREAM_RELEASE};

pub const DELTAS_SCHEMA_VERSION: &str = "2";
/// Canonical hash of `version-deltas.json`; a change to the data is deliberate.
pub const DELTAS_HASH: &str = "122e61da43153928832f0836cccfe1454dec4a725a3400304b1f6ccdeb2a5167";
const DELTAS_JSON: &str = include_str!("../version-deltas.json");

/// Query counters the baseline sums over fragment-instance reports.
const BASELINE_INSTANCE_REPORT_SUM_METRICS: [&str; 2] =
    ["QueryCumulativeCpuTime", "QuerySpillBytes"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CounterTextFormat {
    /// 3.1.1 and later (the baseline): ms/us fractions are zero-padded to three digits.
    Padded,
    /// 3.1.0: the ms/us fraction is the unpadded integer remainder of the next unit.
    LegacyUnpadded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CounterStrategyShape {
    /// `TCounterStrategy` with `min_max_type` (3.3 and later).
    Full,
    /// No `min_max_type`; every counter keeps min/max (3.1–3.2).
    WithoutMinMaxType,
}

/// How a release lays out its text Profile around the counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextStructure {
    /// An operator with a negative plan node id is named `NAME` (a sink in the
    /// plan) or `NAME (pseudo_plan_node_id=N)` (outside the plan) instead of
    /// `NAME (plan_node_id=N)` (3.1.0–3.1.2).
    pub legacy_negative_id_operator_header: bool,
}

impl TextStructure {
    pub const BASELINE: Self = Self {
        legacy_negative_id_operator_header: false,
    };
}

/// Everything a release changes relative to the baseline.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseProfile {
    pub release: String,
    pub line: String,
    /// Delta ids that apply, in file order; empty for the baseline itself.
    pub delta_ids: Vec<String>,
    pub counter_text_format: CounterTextFormat,
    pub text_structure: TextStructure,
    pub counter_strategy: CounterStrategyShape,
    pub plan_advisor: bool,
    pub merged_max_unreliable: bool,
    pub instance_report_sum_metrics: Vec<String>,
    pub output_rows_recipe_available: bool,
    pub profile_list_time_in_session_zone: bool,
    /// Registered metrics this release never emits.
    pub absent_metrics: Vec<String>,
}

impl ReleaseProfile {
    pub fn is_baseline(&self) -> bool {
        self.release == UPSTREAM_RELEASE
    }

    pub fn metric_absent(&self, metric_id: &str) -> bool {
        self.absent_metrics.iter().any(|metric| metric == metric_id)
    }

    pub fn instance_report_sum(&self, metric_id: &str) -> bool {
        self.instance_report_sum_metrics
            .iter()
            .any(|metric| metric == metric_id)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReleaseResolution {
    /// An audited release: the baseline plus its deltas.
    Audited(ReleaseProfile),
    /// A release of a supported line that the audit has not covered (a newer patch).
    NotAudited { line: String },
    /// A release of a line the audit does not cover (2.5, 3.0, anything after
    /// 4.1 until audited), or not a release string.
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeltasError {
    InvalidJson,
    HashMismatch,
    SchemaMismatch,
    BaselineMismatch,
    InvalidRange(String),
    UnknownEffect(String),
}

#[derive(Debug, Deserialize)]
struct DeltasDocument {
    schema_version: String,
    baseline_release: String,
    baseline_commit: String,
    lines: Vec<LineEntry>,
    deltas: Vec<DeltaEntry>,
    absent_metrics: Vec<AbsentMetric>,
}

#[derive(Debug, Deserialize)]
struct LineEntry {
    line: String,
    audited_from: String,
    audited_to: String,
}

#[derive(Debug, Deserialize)]
struct DeltaEntry {
    id: String,
    ranges: Vec<(String, String)>,
    effect: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct AbsentMetric {
    metric: String,
    ranges: Vec<(String, String)>,
}

type Release = (u32, u32, u32);
type ReleaseRange = (Release, Release);

fn parse_release(value: &str) -> Option<Release> {
    let mut parts = value.split('.');
    let mut next = || -> Option<u32> {
        let part = parts.next()?;
        (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| part.parse().ok())?
    };
    let release = (next()?, next()?, next()?);
    parts.next().is_none().then_some(release)
}

fn in_range(release: Release, (from, to): &ReleaseRange) -> bool {
    *from <= release && release <= *to
}

struct Deltas {
    lines: Vec<(String, Release, Release)>,
    deltas: Vec<(
        String,
        Vec<ReleaseRange>,
        serde_json::Map<String, serde_json::Value>,
    )>,
    absent_metrics: Vec<(String, Vec<ReleaseRange>)>,
}

fn ranges(id: &str, ranges: &[(String, String)]) -> Result<Vec<ReleaseRange>, DeltasError> {
    ranges
        .iter()
        .map(
            |(from, to)| match (parse_release(from), parse_release(to)) {
                (Some(from), Some(to)) if from <= to => Ok((from, to)),
                _ => Err(DeltasError::InvalidRange(id.to_owned())),
            },
        )
        .collect()
}

const KNOWN_EFFECTS: [&str; 9] = [
    "text_counter_format",
    "text_structure",
    "capability",
    "counter_strategy",
    "merged_max_unreliable",
    "instance_report_sum_metrics_add",
    "instance_report_sum_metrics_remove",
    "output_rows_recipe",
    "profile_list_time_zone",
];

fn load_deltas() -> Result<Deltas, DeltasError> {
    let value: serde_json::Value =
        serde_json::from_str(DELTAS_JSON).map_err(|_| DeltasError::InvalidJson)?;
    if canonical_hash(&value) != DELTAS_HASH {
        return Err(DeltasError::HashMismatch);
    }
    let document: DeltasDocument =
        serde_json::from_value(value).map_err(|_| DeltasError::InvalidJson)?;
    if document.schema_version != DELTAS_SCHEMA_VERSION {
        return Err(DeltasError::SchemaMismatch);
    }
    if document.baseline_release != UPSTREAM_RELEASE || document.baseline_commit != UPSTREAM_COMMIT
    {
        return Err(DeltasError::BaselineMismatch);
    }
    let mut lines = Vec::new();
    for line in &document.lines {
        let [(from, to)] = ranges(
            &line.line,
            &[(line.audited_from.clone(), line.audited_to.clone())],
        )?[..] else {
            return Err(DeltasError::InvalidRange(line.line.clone()));
        };
        lines.push((line.line.clone(), from, to));
    }
    let mut deltas = Vec::new();
    for delta in document.deltas {
        if let Some(unknown) = delta
            .effect
            .keys()
            .find(|key| !KNOWN_EFFECTS.contains(&key.as_str()))
        {
            return Err(DeltasError::UnknownEffect(format!(
                "{}.{unknown}",
                delta.id
            )));
        }
        deltas.push((
            delta.id.clone(),
            ranges(&delta.id, &delta.ranges)?,
            delta.effect,
        ));
    }
    let absent_metrics = document
        .absent_metrics
        .iter()
        .map(|entry| Ok((entry.metric.clone(), ranges(&entry.metric, &entry.ranges)?)))
        .collect::<Result<_, DeltasError>>()?;
    Ok(Deltas {
        lines,
        deltas,
        absent_metrics,
    })
}

fn deltas() -> Result<&'static Deltas, DeltasError> {
    static DELTAS: OnceLock<Result<Deltas, DeltasError>> = OnceLock::new();
    DELTAS
        .get_or_init(load_deltas)
        .as_ref()
        .map_err(Clone::clone)
}

/// Validates the embedded data; `resolve_release` treats invalid data as "no release supported".
pub fn validate() -> Result<(), DeltasError> {
    deltas().map(|_| ())
}

fn str_field<'a>(
    effect: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a str> {
    effect.get(key).and_then(serde_json::Value::as_str)
}

fn metric_list(effect: &serde_json::Map<String, serde_json::Value>, key: &str) -> Vec<String> {
    effect
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Resolves a reported `x.y.z` release to the baseline plus its deltas.
pub fn resolve_release(value: &str) -> ReleaseResolution {
    let (Some(release), Ok(data)) = (parse_release(value.trim()), deltas()) else {
        return ReleaseResolution::Unsupported;
    };
    let line_name = format!("{}.{}", release.0, release.1);
    let Some((line, _, _)) = data
        .lines
        .iter()
        .find(|(_, from, to)| in_range(release, &(*from, *to)))
    else {
        return if data.lines.iter().any(|(line, _, _)| *line == line_name) {
            ReleaseResolution::NotAudited { line: line_name }
        } else {
            ReleaseResolution::Unsupported
        };
    };
    let mut profile = ReleaseProfile {
        release: format!("{}.{}.{}", release.0, release.1, release.2),
        line: line.clone(),
        delta_ids: Vec::new(),
        counter_text_format: CounterTextFormat::Padded,
        text_structure: TextStructure::BASELINE,
        counter_strategy: CounterStrategyShape::Full,
        plan_advisor: true,
        merged_max_unreliable: false,
        instance_report_sum_metrics: BASELINE_INSTANCE_REPORT_SUM_METRICS
            .iter()
            .map(|metric| (*metric).to_owned())
            .collect(),
        output_rows_recipe_available: true,
        profile_list_time_in_session_zone: false,
        absent_metrics: Vec::new(),
    };
    for (id, delta_ranges, effect) in &data.deltas {
        if !delta_ranges.iter().any(|range| in_range(release, range)) {
            continue;
        }
        profile.delta_ids.push(id.clone());
        if str_field(effect, "text_counter_format") == Some("legacy_unpadded") {
            profile.counter_text_format = CounterTextFormat::LegacyUnpadded;
        }
        if let Some(structure) = effect
            .get("text_structure")
            .and_then(serde_json::Value::as_object)
        {
            for (name, value) in structure {
                let structure = &mut profile.text_structure;
                if (name.as_str(), value.as_str())
                    == ("operator_header", Some("legacy_negative_ids"))
                {
                    structure.legacy_negative_id_operator_header = true;
                }
            }
        }
        if str_field(effect, "counter_strategy") == Some("without_min_max_type") {
            profile.counter_strategy = CounterStrategyShape::WithoutMinMaxType;
        }
        if let Some(available) = effect
            .get("capability")
            .and_then(|capabilities| capabilities.get("plan_advisor"))
            .and_then(serde_json::Value::as_bool)
        {
            profile.plan_advisor = available;
        }
        if effect
            .get("merged_max_unreliable")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            profile.merged_max_unreliable = true;
        }
        for metric in metric_list(effect, "instance_report_sum_metrics_add") {
            if !profile.instance_report_sum(&metric) {
                profile.instance_report_sum_metrics.push(metric);
            }
        }
        let removed = metric_list(effect, "instance_report_sum_metrics_remove");
        profile
            .instance_report_sum_metrics
            .retain(|metric| !removed.contains(metric));
        if str_field(effect, "output_rows_recipe") == Some("unavailable") {
            profile.output_rows_recipe_available = false;
        }
        if str_field(effect, "profile_list_time_zone") == Some("session") {
            profile.profile_list_time_in_session_zone = true;
        }
    }
    profile.absent_metrics = data
        .absent_metrics
        .iter()
        .filter(|(_, metric_ranges)| metric_ranges.iter().any(|range| in_range(release, range)))
        .map(|(metric, _)| metric.clone())
        .collect();
    ReleaseResolution::Audited(profile)
}

/// The counter text format of a reported release; unknown releases read as the
/// baseline, since their metrics are isolated as unsupported anyway.
pub fn counter_text_format(value: &str) -> CounterTextFormat {
    match resolve_release(value) {
        ReleaseResolution::Audited(profile) => profile.counter_text_format,
        _ => CounterTextFormat::Padded,
    }
}

/// The text structure of a reported release; unknown releases read as the
/// baseline, so no legacy spelling is ever accepted for them.
pub fn text_structure(value: &str) -> TextStructure {
    match resolve_release(value) {
        ReleaseResolution::Audited(profile) => profile.text_structure,
        _ => TextStructure::BASELINE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audited(value: &str) -> ReleaseProfile {
        match resolve_release(value) {
            ReleaseResolution::Audited(profile) => profile,
            other => panic!("{value}: {other:?}"),
        }
    }

    #[test]
    fn the_embedded_data_is_valid_and_hashed() {
        assert_eq!(validate(), Ok(()));
    }

    #[test]
    fn the_baseline_has_no_deltas() {
        let baseline = audited("3.5.14");
        assert!(baseline.is_baseline());
        assert!(baseline.delta_ids.is_empty());
        assert_eq!(baseline.counter_text_format, CounterTextFormat::Padded);
        assert_eq!(
            baseline.instance_report_sum_metrics,
            ["QueryCumulativeCpuTime", "QuerySpillBytes"]
        );
        assert!(baseline.absent_metrics.is_empty());
    }

    #[test]
    fn the_text_format_switches_inside_the_3_1_line() {
        assert_eq!(
            audited("3.1.0").counter_text_format,
            CounterTextFormat::LegacyUnpadded
        );
        assert_eq!(
            audited("3.1.1").counter_text_format,
            CounterTextFormat::Padded
        );
        assert_eq!(counter_text_format("9.9.9"), CounterTextFormat::Padded);
    }

    #[test]
    fn the_operator_header_changes_at_3_1_3() {
        let legacy = TextStructure {
            legacy_negative_id_operator_header: true,
        };
        assert_eq!(text_structure("3.1.0"), legacy);
        assert_eq!(text_structure("3.1.2"), legacy);
        assert_eq!(text_structure("3.1.3"), TextStructure::BASELINE);
        assert_eq!(text_structure("3.5.14"), TextStructure::BASELINE);
        // Unsupported releases never accept a legacy spelling.
        assert_eq!(text_structure("3.0.9"), TextStructure::BASELINE);
        assert_eq!(text_structure("2.5.21"), TextStructure::BASELINE);
    }

    #[test]
    fn early_supported_lines_lose_some_capabilities_and_counters() {
        let first = audited("3.1.0");
        assert_eq!(
            first.counter_strategy,
            CounterStrategyShape::WithoutMinMaxType
        );
        assert!(!first.plan_advisor);
        assert!(first.metric_absent("QueryCumulativeScanTime"));
        assert!(!first.output_rows_recipe_available);
        assert!(audited("3.1.4").output_rows_recipe_available);
        assert_eq!(
            audited("3.3.0").counter_strategy,
            CounterStrategyShape::Full
        );
        assert!(!audited("3.3.22").plan_advisor && audited("3.4.0").plan_advisor);
    }

    #[test]
    fn releases_before_3_1_are_unsupported() {
        for release in ["2.5.0", "2.5.21", "3.0.0", "3.0.9"] {
            assert_eq!(
                resolve_release(release),
                ReleaseResolution::Unsupported,
                "{release}"
            );
        }
    }

    #[test]
    fn upstream_defects_and_fixes_change_limitations() {
        assert!(audited("3.3.11").merged_max_unreliable);
        assert!(!audited("3.3.12").merged_max_unreliable);
        assert!(audited("3.4.1").merged_max_unreliable && !audited("3.4.2").merged_max_unreliable);
        assert!(audited("3.3.3").instance_report_sum("QuerySumMemoryUsage"));
        assert!(!audited("3.3.6").instance_report_sum("QuerySumMemoryUsage"));
        assert!(audited("4.0.17").instance_report_sum_metrics.is_empty());
        assert!(audited("4.0.16").instance_report_sum("QueryCumulativeCpuTime"));
        assert!(audited("4.1.7").instance_report_sum_metrics.is_empty());
    }

    #[test]
    fn unaudited_and_unsupported_releases_never_borrow_a_line() {
        assert_eq!(
            resolve_release("4.1.8"),
            ReleaseResolution::NotAudited { line: "4.1".into() }
        );
        assert_eq!(resolve_release("2.4.5"), ReleaseResolution::Unsupported);
        assert_eq!(resolve_release("4.2.0"), ReleaseResolution::Unsupported);
        assert_eq!(resolve_release("3.5"), ReleaseResolution::Unsupported);
        assert_eq!(
            resolve_release("3.5.14-23a56ec"),
            ReleaseResolution::Unsupported
        );
        assert_eq!(resolve_release("03.5.14x"), ReleaseResolution::Unsupported);
    }
}
