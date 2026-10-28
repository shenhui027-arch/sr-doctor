use doctor_contracts::AuditFamilyExecutionDistribution;
use doctor_core::{
    AUDIT_FAMILY_HISTOGRAM_BINS, AuditFamilyDistributionError, AuditFamilyDistributionSource,
    MAX_AUDIT_QUERY_TIME_MS, project_audit_family_distribution,
};

fn source() -> AuditFamilyDistributionSource {
    AuditFamilyDistributionSource {
        population_count: "504".into(),
        eligible_count: "501".into(),
        sql_null_count: "1".into(),
        negative_count: "1".into(),
        overflow_count: "1".into(),
        bucket_counts: ["501", "0", "0", "0", "0", "0", "0", "0"].map(String::from),
    }
}

#[test]
fn distribution_is_full_population_and_counts_stay_exact_text() {
    let AuditFamilyExecutionDistribution::Exact {
        population_count,
        eligible_count,
        buckets,
        ..
    } = project_audit_family_distribution(&source()).unwrap();
    assert_eq!(population_count, "504");
    assert_eq!(eligible_count, "501");
    assert_eq!(buckets.len(), 8);
    assert_eq!(buckets[0].count, "501");
    assert_eq!(
        buckets
            .iter()
            .map(|bucket| bucket.display_label_seconds.as_str())
            .collect::<Vec<_>>(),
        [
            "< 10 ms",
            "10–100 ms",
            "100 ms–1 s",
            "1–10 s",
            "10–60 s",
            "1–5 min",
            "5–60 min",
            "≥ 60 min"
        ]
    );
    let mut large = source();
    large.population_count = "9007199254740996".into();
    large.eligible_count = "9007199254740993".into();
    large.bucket_counts[0] = large.eligible_count.clone();
    let json = serde_json::to_value(project_audit_family_distribution(&large).unwrap()).unwrap();
    assert_eq!(json["eligible_count"], "9007199254740993");
}

#[test]
fn histogram_boundaries_partition_exactly_the_typed_duration_domain() {
    assert_eq!(AUDIT_FAMILY_HISTOGRAM_BINS[0].lower_ms, 0);
    assert_eq!(
        AUDIT_FAMILY_HISTOGRAM_BINS[7].upper_ms,
        MAX_AUDIT_QUERY_TIME_MS + 1
    );
    for bins in AUDIT_FAMILY_HISTOGRAM_BINS.windows(2) {
        assert_eq!(bins[0].upper_ms, bins[1].lower_ms);
    }
    for value in [
        -1,
        0,
        9,
        10,
        99,
        100,
        999,
        1000,
        9999,
        10000,
        59999,
        60000,
        299999,
        300000,
        3599999,
        3600000,
        MAX_AUDIT_QUERY_TIME_MS,
        MAX_AUDIT_QUERY_TIME_MS + 1,
    ] {
        assert_eq!(
            AUDIT_FAMILY_HISTOGRAM_BINS
                .iter()
                .filter(|bin| value >= bin.lower_ms && value < bin.upper_ms)
                .count(),
            usize::from((0..=MAX_AUDIT_QUERY_TIME_MS).contains(&value)),
            "{value}"
        );
    }
}

#[test]
fn exact_empty_is_valid_but_both_coverage_equations_are_required() {
    let mut row = source();
    row.population_count = "0".into();
    row.eligible_count = "0".into();
    row.sql_null_count = "0".into();
    row.negative_count = "0".into();
    row.overflow_count = "0".into();
    row.bucket_counts[0] = "0".into();
    assert!(project_audit_family_distribution(&row).is_ok());
    row.bucket_counts[7] = "1".into();
    assert_eq!(
        project_audit_family_distribution(&row),
        Err(AuditFamilyDistributionError::CoverageMismatch)
    );
    let mut row = source();
    row.population_count = "505".into();
    assert_eq!(
        project_audit_family_distribution(&row),
        Err(AuditFamilyDistributionError::CoverageMismatch)
    );
}

#[test]
fn every_count_rejects_noncanonical_decimal_and_checked_addition_overflow() {
    for invalid in ["", "00", "01", "+1", "-1", " 1", "1 ", "1.0", "1e1", "１"] {
        let mut row = source();
        row.bucket_counts[4] = invalid.into();
        assert_eq!(
            project_audit_family_distribution(&row),
            Err(AuditFamilyDistributionError::InvalidCount),
            "{invalid}"
        );
    }
    for too_big in [
        "18446744073709551616",
        "99999999999999999999999999999999999999",
    ] {
        let mut row = source();
        row.population_count = too_big.into();
        assert_eq!(
            project_audit_family_distribution(&row),
            Err(AuditFamilyDistributionError::CountOverflow)
        );
    }
    let mut row = source();
    row.population_count = u64::MAX.to_string();
    row.eligible_count = u64::MAX.to_string();
    assert_eq!(
        project_audit_family_distribution(&row),
        Err(AuditFamilyDistributionError::CountOverflow)
    );
}
