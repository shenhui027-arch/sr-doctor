//! Cooperative cancellation inside long analysis loops (v0.6-08).
//!
//! Stage boundaries alone left the largest Profiles running for seconds
//! after their deadline. The text parse loop, the runtime projection, the
//! portable facts and the public report assembly now count units of work
//! (lines, operators, metric cells, observations) and poll the caller's stop
//! signal every `POLL_INTERVAL` units. A stopped analysis returns
//! `Cancelled` and drops what it has built: there is no partial result.

use std::{cell::Cell, cmp::Ordering};

/// Units of work between two polls. One unit costs about 1-4 us in a release
/// build, so a poll lands every few milliseconds; one poll is an indirect call.
pub(crate) const POLL_INTERVAL: u32 = 256;

/// Caller-owned stop signal. `never()` keeps the uncancellable behaviour of
/// the plain entry points (`adapt_input`, `assemble_public_report`).
pub struct CancelSignal<'a> {
    stop: Option<&'a dyn Fn() -> bool>,
    units: Cell<u32>,
}

impl<'a> CancelSignal<'a> {
    /// `stop` returns `true` once the analysis must stop; it is polled from
    /// the analysing thread only.
    pub fn new(stop: &'a dyn Fn() -> bool) -> Self {
        Self {
            stop: Some(stop),
            units: Cell::new(0),
        }
    }

    pub fn never() -> CancelSignal<'static> {
        CancelSignal {
            stop: None,
            units: Cell::new(0),
        }
    }

    /// Counts one unit of work and polls every `POLL_INTERVAL` units.
    pub(crate) fn tick(&self) -> Result<(), Cancelled> {
        let units = self.units.get().wrapping_add(1);
        self.units.set(units);
        if units.is_multiple_of(POLL_INTERVAL) {
            self.check()
        } else {
            Ok(())
        }
    }

    /// Polls now, at a step boundary or before an uncountable step.
    pub(crate) fn check(&self) -> Result<(), Cancelled> {
        match self.stop {
            Some(stop) if stop() => Err(Cancelled),
            _ => Ok(()),
        }
    }
}

/// Frees `items` one at a time, one unit each. Freeing the largest consumed
/// collections takes hundreds of milliseconds under load; polling meanwhile
/// sees a stop at once, and whatever is left is freed while returning.
pub(crate) fn drop_polled<I: IntoIterator>(
    items: I,
    cancel: &CancelSignal<'_>,
) -> Result<(), Cancelled> {
    for item in items {
        drop(item);
        cancel.tick()?;
    }
    Ok(())
}

/// Elements per chunk in `sort_by_polled`; one chunk sorts in a few milliseconds.
const SORT_CHUNK: usize = 16 * 1024;

/// `items.sort_by(order)`, polling between steps. Sorting the 250,000 metric
/// facts of the largest Profiles takes up to 200 ms. Chunks are stable-sorted
/// first, then neighbouring sorted runs are merged pairwise (a stable sort of
/// two sorted runs is a merge); the result is exactly that of one `sort_by`
/// (ties keep their input order).
pub(crate) fn sort_by_polled<T>(
    items: &mut [T],
    order: impl Fn(&T, &T) -> Ordering,
    cancel: &CancelSignal<'_>,
) -> Result<(), Cancelled> {
    let mut width = SORT_CHUNK;
    for chunk in items.chunks_mut(width) {
        cancel.check()?;
        chunk.sort_by(&order);
    }
    while width < items.len() {
        for pair in items.chunks_mut(2 * width) {
            cancel.check()?;
            pair.sort_by(&order);
        }
        width *= 2;
    }
    cancel.check()
}

/// The stop signal was observed; the analysis discarded its work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cancelled;

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn tick_polls_once_per_interval_and_check_polls_every_time() {
        let polls = Cell::new(0);
        let stop = || {
            polls.set(polls.get() + 1);
            false
        };
        let signal = CancelSignal::new(&stop);
        for _ in 0..POLL_INTERVAL * 3 {
            signal.tick().unwrap();
        }
        assert_eq!(polls.get(), 3);
        signal.check().unwrap();
        assert_eq!(polls.get(), 4);
    }

    #[test]
    fn sort_by_polled_matches_one_stable_sort_and_polls_per_chunk() {
        // Many equal keys across chunk borders; the index exposes tie order.
        let items = (0..SORT_CHUNK * 5 + 123)
            .map(|index| ((index * 7919) % 1000, index))
            .collect::<Vec<_>>();
        let by_key = |left: &(usize, usize), right: &(usize, usize)| left.0.cmp(&right.0);
        let mut expected = items.clone();
        expected.sort_by(by_key);
        let polls = Cell::new(0);
        let stop = || {
            polls.set(polls.get() + 1);
            false
        };
        let mut sorted = items.clone();
        sort_by_polled(&mut sorted, by_key, &CancelSignal::new(&stop)).unwrap();
        assert_eq!(sorted, expected);
        // One poll per chunk, per merged pair (3 + 2 + 1) and at the end.
        assert_eq!(polls.get(), 6 + 6 + 1);

        let stop = || true;
        let mut cancelled = items;
        assert_eq!(
            sort_by_polled(&mut cancelled, by_key, &CancelSignal::new(&stop)),
            Err(Cancelled)
        );
    }

    #[test]
    fn a_raised_signal_is_reported_and_never_is_never_cancelled() {
        let stop = || true;
        let signal = CancelSignal::new(&stop);
        assert_eq!(signal.check(), Err(Cancelled));
        let never = CancelSignal::never();
        for _ in 0..POLL_INTERVAL * 2 {
            never.tick().unwrap();
        }
        never.check().unwrap();
    }
}
