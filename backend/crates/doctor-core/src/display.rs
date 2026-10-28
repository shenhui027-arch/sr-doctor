//! The product's one display policy for measured quantities (v0.6-09).
//!
//! Every byte, duration and count the backend renders for a reader goes
//! through here, and the frontend's `lib/duration-display.ts` implements the
//! same rules; both are checked against `fixtures/expected/display-units.json`.
//!
//! - Durations: ns / μs / ms / s, base 1000. A value that renders as 60 s or
//!   more reads as `41m 12s` (seconds half-up), and from 3600 s as `1h 08m`
//!   (minutes half-up), the way StarRocks prints long durations.
//! - Bytes: B / KB / MB / GB / … with StarRocks' binary meaning (base 1024,
//!   1 MB = 1,048,576 bytes), the units StarRocks Profile and ANALYZE PROFILE
//!   print, so a value reads the same here as in the raw Profile.
//! - At most three decimals, half-up, trailing zeros trimmed; a rounded value
//!   equal to the base carries to the next unit.
//! - Counts are grouped by thousands. Negative values use U+2212.
//!
//! Integer arithmetic keeps values above 2^53 exact. This only presents typed,
//! normalized integers; it never parses Profile text.

const DURATION_UNITS: [&str; 4] = ["ns", "μs", "ms", "s"];
const BYTE_UNITS: [&str; 7] = ["B", "KB", "MB", "GB", "TB", "PB", "EB"];

pub fn duration_ns(value: i128) -> String {
    const SECOND: u128 = 1_000_000_000;
    let magnitude = value.unsigned_abs();
    // Below this the three-decimal seconds text stays under "60 s".
    if magnitude < 59_999_500_000 {
        return adaptive(value, &DURATION_UNITS, 1_000);
    }
    let sign = if value < 0 { "−" } else { "" };
    let seconds = magnitude / SECOND + u128::from(magnitude % SECOND >= SECOND / 2);
    if seconds < 3_600 {
        return format!("{sign}{}m {:02}s", seconds / 60, seconds % 60);
    }
    let minute = 60 * SECOND;
    let minutes = magnitude / minute + u128::from(magnitude % minute >= minute / 2);
    format!("{sign}{}h {:02}m", minutes / 60, minutes % 60)
}

pub fn bytes(value: i128) -> String {
    adaptive(value, &BYTE_UNITS, 1_024)
}

pub fn count(value: i128) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3 + 3);
    if value < 0 {
        grouped.push('−');
    }
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// A dense count for tables and cards: grouped below 1,000, then K with no
/// decimals, M with one, B and T with two (half-up, carrying at 1000).
pub fn count_compact(value: i128) -> String {
    const UNITS: [(&str, u128, u32); 4] = [
        ("K", 1_000, 0),
        ("M", 1_000_000, 1),
        ("B", 1_000_000_000, 2),
        ("T", 1_000_000_000_000, 2),
    ];
    let magnitude = value.unsigned_abs();
    if magnitude < 1_000 {
        return count(value);
    }
    let sign = if value < 0 { "−" } else { "" };
    let mut index = UNITS
        .iter()
        .rposition(|(_, factor, _)| magnitude >= *factor)
        .unwrap_or(0);
    loop {
        let (unit, factor, decimals) = UNITS[index];
        let scale = 10_u128.pow(decimals);
        let scaled =
            magnitude / factor * scale + ((magnitude % factor) * scale + factor / 2) / factor;
        let whole = scaled / scale;
        if whole >= 1_000 && index + 1 < UNITS.len() {
            index += 1;
            continue;
        }
        let fraction = scaled % scale;
        return if decimals == 0 {
            format!("{sign}{whole} {unit}")
        } else {
            format!(
                "{sign}{whole}.{fraction:0width$} {unit}",
                width = decimals as usize
            )
        };
    }
}

/// `lower – upper`, or a single value when both ends render the same.
pub fn range(lower: i128, upper: i128, format: fn(i128) -> String) -> String {
    let (lower, upper) = (format(lower), format(upper));
    if lower == upper {
        lower
    } else {
        format!("{lower} – {upper}")
    }
}

/// A byte interval. One no wider than two display steps is the rounding of a
/// value StarRocks printed (`576.647 MB` is [576.6465, 576.6475) MB), so it reads
/// as that value, like the source, instead of two adjacent ends; the exact
/// interval stays in the DTO.
pub fn bytes_range(lower: i128, upper: i128) -> String {
    narrow_range(lower, upper, 1_024, bytes)
}

/// A duration interval, with the same narrow-interval rule as `bytes_range`.
pub fn duration_ns_range(lower: i128, upper: i128) -> String {
    narrow_range(lower, upper, 1_000, duration_ns)
}

fn narrow_range(lower: i128, upper: i128, base: u128, format: fn(i128) -> String) -> String {
    let (low_text, high_text) = (format(lower), format(upper));
    if low_text == high_text {
        return low_text;
    }
    let middle = lower + (upper - lower) / 2;
    let magnitude = middle.unsigned_abs();
    let mut factor = 1_u128;
    while magnitude >= factor.saturating_mul(base) && factor < u128::MAX / base {
        factor *= base;
    }
    // One display step is a thousandth of the unit the middle renders in.
    if (upper - lower).unsigned_abs().saturating_mul(1_000) <= factor.saturating_mul(2) {
        format(middle)
    } else {
        format!("{low_text} – {high_text}")
    }
}

fn adaptive(value: i128, units: &[&str], base: u128) -> String {
    let magnitude = value.unsigned_abs();
    let factor_at = |index: usize| base.pow(u32::try_from(index).unwrap_or(u32::MAX));
    let mut index = (0..units.len())
        .rev()
        .find(|index| magnitude >= factor_at(*index))
        .unwrap_or(0);
    loop {
        let factor = factor_at(index);
        // Whole and fraction separately: the remainder is below the factor, so
        // `remainder · 1000` cannot overflow even for |i128::MIN|.
        let mut whole = magnitude / factor;
        let mut fraction = ((magnitude % factor) * 1_000 + factor / 2) / factor;
        if fraction == 1_000 {
            whole += 1;
            fraction = 0;
        }
        if whole >= base && index + 1 < units.len() {
            index += 1;
            continue;
        }
        let sign = if value < 0 { "−" } else { "" };
        let decimal = if fraction == 0 {
            String::new()
        } else {
            format!(".{fraction:03}").trim_end_matches('0').to_owned()
        };
        return format!("{sign}{whole}{decimal} {}", units[index]);
    }
}
