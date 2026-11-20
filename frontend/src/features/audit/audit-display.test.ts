import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { AuditMetricFact, AuditMetricId, AuditMetricState } from "../../../../contracts/generated/doctor-contracts";
import { AuditMetricValue } from "./AuditMetricValue";
import { auditDigestUnavailable, auditMillisecondsText, auditScientific, metricRawText, metricText, metricTone, positiveCount } from "./audit-display";

function fact(metric_id: AuditMetricId, state: AuditMetricState, value: string | null, unit: string | null, canonical = value): AuditMetricFact {
  return {
    metric_id,
    input: value === null ? { state: "sql_null" } : { state: "text", value },
    state,
    parser_state: state === "exact" ? "exact" : null,
    canonical_number: canonical === null ? null : { kind: "integer", value: canonical, scale: null },
    canonical_unit: canonical === null ? null : unit,
  };
}

describe("audit display formatting", () => {
  it("keeps integers above 2^53 exact and performs four-significant-digit half-up carry", () => {
    expect(auditScientific("9007199254740993", 0)).toBe("9.007e+15");
    expect(auditScientific("99995", 4)).toBe("1.000e+1");
    expect(auditScientific("1", 9)).toBe("1.000e-9");
    expect(auditScientific("0", 9)).toBe("0.000e+0");
  });

  it("adapts durations and keeps StarRocks binary GB for bytes without turning tiny values into zero", () => {
    expect(metricText(fact("query_time", "exact", "1", "ns"))).toBe("1 ns");
    expect(metricText(fact("scan_bytes", "exact", "1", "byte"))).toBe("1 B");
    expect(metricText(fact("query_time", "exact", "0", "ns"))).toBe("0 ns");
    expect(metricText(fact("query_time", "exact", "619380", "ns"))).toBe("619.38 μs");
    expect(metricText(fact("scan_rows", "exact", "1234567", "item"))).toBe("1,234,567");
    expect(metricText(fact("scan_rows", "exact", "16", "item"))).toBe("16");
    expect(metricText(fact("scan_bytes", "exact", "1073741824", "byte"))).toBe("1 GB");
    expect(metricText(fact("scan_bytes", "exact", "1073741824", "byte"))).not.toContain("GiB");
  });

  it("retains non-exact states and raw numeric text rather than presenting a guessed value", () => {
    const unsafe = fact("scan_bytes", "precision_unsafe", "9007199254740993", "byte");
    expect(metricText(unsafe)).toBe("8 PB (precision_unsafe)");
    expect(metricRawText(unsafe)).toBe("9007199254740993 byte (precision_unsafe)");
    expect(metricText(fact("query_time", "missing", null, null, null))).toBe("— (missing)");
    expect(metricText(fact("query_time", "negative_sentinel", "-1", null, null))).toBe("-1 (negative_sentinel)");
    expect(metricText(fact("query_time", "overflow", "999999999999999999999", null, null))).toBe("999999999999999999999 (overflow)");
    expect(metricText(fact("query_time", "invalid", "not-a-number", null, null))).toBe("not-a-number (invalid)");
    expect(metricText(null)).toBe("—");
  });

  it("uses raw text for unsupported units and keeps exact raw values available for tooltips", () => {
    const unknownUnit = fact("scan_rows", "exact", "42", "row");
    expect(metricText(unknownUnit)).toBe("42 row");
    expect(metricRawText(unknownUnit)).toBe("42 row");
    expect(metricRawText(fact("query_time", "exact", "7", "ns"))).toBe("7 ns");
  });

  it("adapts server-provided elapsed milliseconds without losing zero or invalid state", () => {
    expect(auditMillisecondsText("1")).toBe("1 ms");
    expect(auditMillisecondsText("0")).toBe("0 ns");
    expect(auditMillisecondsText("9007199254740998")).toBe("2501999792h 59m");
    expect(auditMillisecondsText("-1")).toBe("-1 ms");
    expect(auditMillisecondsText("unknown")).toBe("unknown ms");
  });
});

describe("audit coverage counters", () => {
  it("treats decimal zero as a real zero, not a warning", () => {
    expect(positiveCount("0")).toBe(false);
    expect(positiveCount("000")).toBe(false);
    expect(positiveCount("12")).toBe(true);
    expect(positiveCount(undefined)).toBe(false);
    expect(positiveCount("-1")).toBe(false);
  });
  it("reports digest unavailability only when every retained event lacks a digest", () => {
    expect(auditDigestUnavailable({ retained_event_count: "16", missing_digest_count: "16" })).toBe(true);
    expect(auditDigestUnavailable({ retained_event_count: "16", missing_digest_count: "3" })).toBe(false);
    expect(auditDigestUnavailable({ retained_event_count: "0", missing_digest_count: "0" })).toBe(false);
    expect(auditDigestUnavailable(null)).toBe(false);
  });
});

describe("audit metric display tone and StateStamp rendering (correctness fix #1)", () => {
  it("keeps exact zero plain and gives missing, invalid, negative_sentinel, overflow and unavailable distinct StateStamps", () => {
    const exactZero = fact("query_time", "exact", "0", "ns");
    const missing = fact("cpu_cost", "missing", null, null);
    const invalid = fact("query_time", "invalid", "not-a-number", null, null);
    const sentinel = fact("scan_rows", "negative_sentinel", "-1", null, null);
    const overflow = fact("memory_cost", "overflow", "999999999999999999999", null, null);
    const unavailable: AuditMetricFact = { metric_id: "scan_bytes", input: { state: "unavailable", reason: "source_column_absent" }, state: "unavailable", parser_state: null, canonical_number: null, canonical_unit: null };

    expect(metricTone(exactZero)).toBe("exact");
    expect(metricTone(fact("scan_bytes", "precision_unsafe", "9007199254740993", "byte"))).toBe("qualified");
    expect(metricTone(missing)).toBe("missing");
    expect(metricTone(null)).toBe("missing");
    expect(metricTone(invalid)).toBe("invalid");
    expect(metricTone(sentinel)).toBe("negative_sentinel");
    expect(metricTone(overflow)).toBe("overflow");
    expect(metricTone(unavailable)).toBe("unavailable");

    const renderValue = (value: AuditMetricFact | null) => renderToStaticMarkup(createElement(AuditMetricValue, { value }));
    expect(renderValue(exactZero)).toBe("0 ns");
    expect(renderValue(missing)).toContain('data-honest-state="missing"');
    expect(renderValue(invalid)).toContain('data-honest-state="invalid"');
    expect(renderValue(invalid)).not.toContain(">Missing<");
    expect(renderValue(sentinel)).toContain('data-honest-state="negative_sentinel"');
    expect(renderValue(sentinel)).not.toContain(">Missing<");
    expect(renderValue(overflow)).toContain('data-honest-state="overflow"');
    expect(renderValue(overflow)).not.toContain(">Missing<");
    expect(renderValue(unavailable)).toContain('data-honest-state="unavailable"');
    expect(renderValue(unavailable)).not.toContain(">Missing<");
  });
});
