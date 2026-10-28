//! Cooperative cancellation inside adapt and assemble (v0.6-08).
//!
//! * Without a raised signal the cancellable entry points return exactly what
//!   the plain ones return (same output, same `facts_hash`).
//! * A signal raised at any poll stops adapt, resolve or assemble at that
//!   poll with the explicit cancellation error; there is no partial output or
//!   report.
//! * Polls are spread through the work: at least one per `POLL_INTERVAL`
//!   (256) text lines, metric cells or observations, so a stop is seen within
//!   milliseconds instead of at the next stage boundary.

#[path = "support/scale_profile.rs"]
mod scale_profile;

use std::cell::Cell;
use std::time::{Duration, Instant};

use doctor_contracts::RequestedSourceKind;
use doctor_core::{
    ANALYSIS_CANCELLED, AdapterContext, AdapterLimits, AdapterOutput, CancelSignal, InputEnvelope,
    PublicReportError, VersionResolutionError, adapt_input, adapt_input_cancellable,
    assemble_public_report, assemble_public_report_cancellable, evaluate_rules,
    resolve_version_and_graph, resolve_version_and_graph_cancellable, v01_analysis_receipt,
};
use scale_profile::{BULK_PIPELINES, Shape, metric_dense};

const POLL_INTERVAL: usize = 256;

fn profile() -> String {
    let (text, _, _) = Shape {
        fragments: 6,
        bulk_pipelines: BULK_PIPELINES,
    }
    .profile();
    metric_dense(&text)
}

fn input(text: &str) -> InputEnvelope {
    InputEnvelope::new(RequestedSourceKind::TextProfile, text.as_bytes().to_vec())
}

fn context() -> AdapterContext {
    AdapterContext::new(v01_analysis_receipt()).expect("receipt context")
}

/// Raised from the `after`-th poll on; counts every poll.
struct Countdown {
    after: usize,
    polls: Cell<usize>,
}

impl Countdown {
    fn new(after: usize) -> Self {
        Self {
            after,
            polls: Cell::new(0),
        }
    }

    fn poll(&self) -> bool {
        self.polls.set(self.polls.get() + 1);
        self.polls.get() >= self.after
    }
}

fn resolved(mut output: AdapterOutput) -> AdapterOutput {
    let evidence = output.graph_evidence.clone();
    resolve_version_and_graph(&mut output, evidence).expect("resolves");
    evaluate_rules(&mut output).expect("rules evaluate");
    output
}

/// Polls of one uncancelled run, sampled at the first, last and evenly
/// spaced points in between.
fn sample_points(polls: usize) -> Vec<usize> {
    let mut points = (1..=polls).step_by((polls / 12).max(1)).collect::<Vec<_>>();
    points.push(polls);
    points
}

#[test]
fn uncancelled_runs_match_the_plain_entry_points() {
    let text = profile();
    let limits = AdapterLimits::default();
    let plain = adapt_input(input(&text), &limits, &context()).expect("adapts");
    let never = CancelSignal::never();
    let polled = Countdown::new(usize::MAX);
    let stop = || polled.poll();
    let signal = CancelSignal::new(&stop);
    let cancellable =
        adapt_input_cancellable(input(&text), &limits, &context(), &signal).expect("adapts");
    assert_eq!(cancellable, plain);
    assert_eq!(
        adapt_input_cancellable(input(&text), &limits, &context(), &never).expect("adapts"),
        plain
    );

    let plain_report = assemble_public_report(resolved(plain)).expect("assembles");
    let report =
        assemble_public_report_cancellable(resolved(cancellable), &signal).expect("assembles");
    assert_eq!(report.facts_hash, plain_report.facts_hash);
    assert_eq!(report, plain_report);
}

#[test]
fn adapt_stops_at_any_poll_with_the_cancellation_error() {
    let text = profile();
    let lines = text.lines().count();
    let limits = AdapterLimits::default();
    let counting = Countdown::new(usize::MAX);
    let stop = || counting.poll();
    adapt_input_cancellable(input(&text), &limits, &context(), &CancelSignal::new(&stop))
        .expect("adapts");
    let polls = counting.polls.get();
    assert!(
        polls >= lines / POLL_INTERVAL,
        "{polls} polls for {lines} lines"
    );
    for after in sample_points(polls) {
        let countdown = Countdown::new(after);
        let stop = || countdown.poll();
        let error =
            adapt_input_cancellable(input(&text), &limits, &context(), &CancelSignal::new(&stop))
                .expect_err("cancelled adapt has no output");
        assert!(error.is_cancelled(), "poll {after}: {error:?}");
        assert_eq!(error.code, ANALYSIS_CANCELLED);
        assert_eq!(countdown.polls.get(), after, "stops at the raised poll");
    }
}

#[test]
fn assemble_stops_at_any_poll_with_the_cancellation_error() {
    let text = profile();
    let limits = AdapterLimits::default();
    let output = resolved(adapt_input(input(&text), &limits, &context()).expect("adapts"));
    let metric_cells = output.ir.metric_observations.len();
    let counting = Countdown::new(usize::MAX);
    let stop = || counting.poll();
    assemble_public_report_cancellable(output.clone(), &CancelSignal::new(&stop))
        .expect("assembles");
    let polls = counting.polls.get();
    // Index, refs, rows, match keys and facts each walk every cell.
    assert!(
        polls >= 4 * metric_cells / POLL_INTERVAL,
        "{polls} polls for {metric_cells} observations"
    );
    for after in sample_points(polls) {
        let countdown = Countdown::new(after);
        let stop = || countdown.poll();
        let error = assemble_public_report_cancellable(output.clone(), &CancelSignal::new(&stop))
            .expect_err("cancelled assembly has no report");
        assert_eq!(error, PublicReportError::Cancelled, "poll {after}");
        assert_eq!(countdown.polls.get(), after, "stops at the raised poll");
    }
}

#[test]
fn resolve_stops_at_any_poll_with_the_cancellation_error() {
    let text = profile();
    let limits = AdapterLimits::default();
    let adapted = adapt_input(input(&text), &limits, &context()).expect("adapts");
    let source_refs = adapted.source_refs.len();
    let mut plain = adapted.clone();
    let evidence = plain.graph_evidence.clone();
    resolve_version_and_graph(&mut plain, evidence).expect("resolves");

    let counting = Countdown::new(usize::MAX);
    let stop = || counting.poll();
    let mut output = adapted.clone();
    let evidence = output.graph_evidence.clone();
    resolve_version_and_graph_cancellable(&mut output, evidence, &CancelSignal::new(&stop))
        .expect("resolves");
    assert_eq!(output, plain);
    let polls = counting.polls.get();
    assert!(
        polls >= source_refs / POLL_INTERVAL,
        "{polls} polls for {source_refs} source refs"
    );
    for after in sample_points(polls) {
        let countdown = Countdown::new(after);
        let stop = || countdown.poll();
        let mut output = adapted.clone();
        let evidence = output.graph_evidence.clone();
        let error =
            resolve_version_and_graph_cancellable(&mut output, evidence, &CancelSignal::new(&stop))
                .expect_err("cancelled resolution");
        assert_eq!(error, VersionResolutionError::Cancelled, "poll {after}");
        assert_eq!(countdown.polls.get(), after, "stops at the raised poll");
    }
}

/// Records the longest stretch between two polls (and from the start / to the
/// end of the analysis) and the stage it was in.
struct GapRecorder {
    last: Cell<Instant>,
    stage: Cell<&'static str>,
    longest: Cell<(Duration, &'static str)>,
}

impl GapRecorder {
    fn new() -> Self {
        Self {
            last: Cell::new(Instant::now()),
            stage: Cell::new("adapt"),
            longest: Cell::new((Duration::ZERO, "adapt")),
        }
    }

    fn poll(&self) -> bool {
        let now = Instant::now();
        let gap = now - self.last.replace(now);
        if gap > self.longest.get().0 {
            self.longest.set((gap, self.stage.get()));
        }
        false
    }

    /// A stage boundary: the API polls its lease there too.
    fn enter(&self, stage: &'static str) {
        self.poll();
        self.stage.set(stage);
    }
}

/// The largest Profiles of both shapes the default budget admits (the same
/// shapes as `topology_scale.rs`) are polled at least every 150 ms in a release
/// build, from the first line to the finished report. Stage-boundary polling
/// alone left gaps of 1-2 s here; a loop without its poll shows up as a gap of
/// hundreds of milliseconds. A missing poll repeats in every run while a load
/// spike from other processes does not, so each shape keeps the smallest of
/// `RUNS` longest gaps (quiet machine about 40 ms, a busy one up to 140 ms).
/// Release builds only, like the other timing guards.
#[test]
#[cfg_attr(debug_assertions, ignore = "timing guard for release builds")]
fn the_largest_default_budget_profiles_are_polled_at_least_every_150_ms() {
    const MAX_GAP: Duration = Duration::from_millis(150);
    const RUNS: usize = 3;
    let limits = AdapterLimits::default();
    for (name, fragments, dense) in [("operator-heavy", 590, false), ("metric-dense", 170, true)] {
        let (text, _, _) = Shape {
            fragments,
            bulk_pipelines: BULK_PIPELINES,
        }
        .profile();
        let text = if dense { metric_dense(&text) } else { text };
        let (gap, stage) = (0..RUNS)
            .map(|_| {
                let gaps = GapRecorder::new();
                let stop = || gaps.poll();
                let cancel = CancelSignal::new(&stop);
                let mut output =
                    adapt_input_cancellable(input(&text), &limits, &context(), &cancel)
                        .expect("adapts");
                gaps.enter("resolve");
                let evidence = output.graph_evidence.clone();
                resolve_version_and_graph_cancellable(&mut output, evidence, &cancel)
                    .expect("resolves");
                gaps.enter("rules");
                evaluate_rules(&mut output).expect("rules evaluate");
                gaps.enter("assemble");
                let report =
                    assemble_public_report_cancellable(output, &cancel).expect("assembles");
                gaps.enter("done");
                drop(report);
                let longest = gaps.longest.get();
                eprintln!(
                    "{name}: longest stretch without a poll {:?} in {}",
                    longest.0, longest.1
                );
                longest
            })
            .min_by_key(|(gap, _)| *gap)
            .expect("at least one run");
        assert!(
            gap < MAX_GAP,
            "{name}: {gap:?} without a poll in {stage} in every run"
        );
    }
}
