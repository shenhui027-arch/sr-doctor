//! A mergeable log-linear histogram for hourly history buckets. Each power of
//! two is split into 8 bins, so a quantile is known to within one bin: the
//! reported value is that bin's midpoint and its relative error is at most
//! 1/16. Counts are exact; only quantiles are approximate, and every caller
//! labels them so.
use std::collections::BTreeMap;

const SUB_BINS: u64 = 8;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LogHistogram {
    bins: BTreeMap<u16, u64>,
}

fn index(value: u64) -> u16 {
    if value == 0 {
        return 0;
    }
    let exponent = 63 - u64::from(value.leading_zeros());
    let sub = if exponent >= 3 {
        (value >> (exponent - 3)) & (SUB_BINS - 1)
    } else {
        (value << (3 - exponent)) & (SUB_BINS - 1)
    };
    (1 + exponent * SUB_BINS + sub) as u16
}

/// `[lower, upper)` of a bin; the last bin's upper bound saturates.
fn bounds(index: u16) -> (u64, u64) {
    if index == 0 {
        return (0, 1);
    }
    let exponent = u64::from(index - 1) / SUB_BINS;
    let sub = u64::from(index - 1) % SUB_BINS;
    let base = 1u128 << exponent;
    let lower = base + base * u128::from(sub) / u128::from(SUB_BINS);
    // Below 8 the integer bins are narrower than one unit; keep them non-empty.
    let upper = (base + base * u128::from(sub + 1) / u128::from(SUB_BINS)).max(lower + 1);
    (lower as u64, upper.min(u128::from(u64::MAX)) as u64)
}

impl LogHistogram {
    pub fn record(&mut self, value: u64) {
        *self.bins.entry(index(value)).or_default() += 1;
    }

    pub fn merge(&mut self, other: &LogHistogram) {
        for (bin, count) in &other.bins {
            *self.bins.entry(*bin).or_default() += count;
        }
    }

    pub fn count(&self) -> u64 {
        self.bins.values().sum()
    }

    /// The bin holding the `q` quantile (nearest rank), as `[lower, upper)`
    /// and its midpoint. `None` for an empty histogram.
    pub fn quantile(&self, q: f64) -> Option<(u64, u64, u64)> {
        let total = self.count();
        if total == 0 {
            return None;
        }
        let rank = ((q.clamp(0.0, 1.0) * total as f64).ceil() as u64).max(1);
        let mut seen = 0;
        for (bin, count) in &self.bins {
            seen += count;
            if seen >= rank {
                let (lower, upper) = bounds(*bin);
                let mid = if upper > lower + 1 {
                    lower + (upper - lower) / 2
                } else {
                    lower
                };
                return Some((lower, upper, mid));
            }
        }
        None
    }

    /// Sparse encoding: varint pairs of (bin delta, count).
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut previous = 0u16;
        for (bin, count) in &self.bins {
            varint(&mut out, u64::from(bin - previous));
            varint(&mut out, *count);
            previous = *bin;
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<LogHistogram> {
        let mut bins = BTreeMap::new();
        let (mut at, mut bin) = (0usize, 0u64);
        while at < bytes.len() {
            let (delta, used) = read_varint(&bytes[at..])?;
            at += used;
            let (count, used) = read_varint(&bytes[at..])?;
            at += used;
            bin += delta;
            if bin > 1 + 63 * SUB_BINS + SUB_BINS || count == 0 {
                return None;
            }
            bins.insert(bin as u16, count);
        }
        Some(LogHistogram { bins })
    }
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn read_varint(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for (used, byte) in bytes.iter().enumerate().take(10) {
        value |= u64::from(byte & 0x7f) << (7 * used);
        if byte & 0x80 == 0 {
            return Some((value, used + 1));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles_are_within_one_sixteenth_and_zero_is_exact() {
        let mut histogram = LogHistogram::default();
        for value in [0, 0, 0] {
            histogram.record(value);
        }
        assert_eq!(histogram.quantile(0.5), Some((0, 1, 0)));
        let mut histogram = LogHistogram::default();
        for value in 1..=1000u64 {
            histogram.record(value * 37);
        }
        let (lower, upper, mid) = histogram.quantile(0.5).unwrap();
        assert!(lower <= 18_500 && 18_500 < upper);
        assert!((mid as f64 - 18_500.0).abs() / 18_500.0 <= 1.0 / 16.0);
        assert!(histogram.quantile(0.0).is_some());
        assert!(LogHistogram::default().quantile(0.5).is_none());
    }

    #[test]
    fn merging_and_encoding_round_trip() {
        let (mut a, mut b) = (LogHistogram::default(), LogHistogram::default());
        for value in [5, 900, 70_000, u64::MAX] {
            a.record(value);
        }
        for value in [5, 6, 1] {
            b.record(value);
        }
        let mut merged = a.clone();
        merged.merge(&b);
        assert_eq!(merged.count(), 7);
        assert_eq!(LogHistogram::decode(&merged.encode()), Some(merged.clone()));
        assert_eq!(LogHistogram::decode(&[0x80]), None);
        assert_eq!(LogHistogram::decode(&[5, 0]), None);
        // Every bin's bounds are ordered and the top bin does not overflow.
        for value in [1u64, 2, 3, 7, 8, 9, 1 << 40, u64::MAX] {
            let (lower, upper) = bounds(index(value));
            assert!(
                lower <= value && (value < upper || upper == u64::MAX),
                "{value}"
            );
        }
    }
}
