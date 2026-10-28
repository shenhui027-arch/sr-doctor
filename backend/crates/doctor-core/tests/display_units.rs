//! The backend display policy against the shared vectors the frontend also
//! checks (`frontend/src/lib/duration-display.test.ts`).

use doctor_core::display;
use serde_json::Value;

#[test]
fn backend_display_matches_the_shared_unit_vectors() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/expected/display-units.json"
    );
    let vectors: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(vectors["schema_version"], "sr-doctor.display-units/v1");
    let cases = vectors["cases"].as_array().unwrap();
    assert!(cases.len() >= 20);
    for case in cases {
        let value: i128 = case["value"].as_str().unwrap().parse().unwrap();
        let actual = match case["kind"].as_str().unwrap() {
            "duration_ns" => display::duration_ns(value),
            "bytes" => display::bytes(value),
            "count" => display::count(value),
            "count_compact" => display::count_compact(value),
            other => panic!("unknown kind {other}"),
        };
        assert_eq!(actual, case["text"].as_str().unwrap(), "{case}");
    }
}

#[test]
fn extreme_magnitudes_do_not_overflow() {
    assert!(display::duration_ns(i128::MAX).ends_with("h 42m"));
    assert!(display::duration_ns(i128::MIN).starts_with('−'));
    assert!(display::bytes(i128::MIN).starts_with('−'));
    assert_eq!(
        display::range(1_048_576, 2_097_151, display::bytes),
        "1 MB – 2 MB"
    );
    assert_eq!(display::range(5, 5, display::count), "5");
}

#[test]
fn rounding_intervals_read_as_one_approximate_value() {
    // "576.647 MB" printed by StarRocks: [576.6465, 576.6475) MB in bytes.
    assert_eq!(display::bytes_range(604_657_680, 604_658_729), "576.647 MB");
    // A real spread stays a range.
    assert_eq!(display::bytes_range(1 << 20, 2 << 20), "1 MB – 2 MB");
    assert_eq!(display::bytes_range(512, 512), "512 B");
    assert_eq!(
        display::duration_ns_range(48_240_500, 48_241_499),
        "48.241 ms"
    );
    assert_eq!(
        display::duration_ns_range(48_240_000, 48_242_000),
        "48.241 ms"
    );
    assert_eq!(
        display::duration_ns_range(1_000_000, 3_000_000),
        "1 ms – 3 ms"
    );
}
