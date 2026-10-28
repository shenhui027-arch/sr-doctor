import { describe, expect, it } from "vitest";
import type { BoundedInterval, CanonicalNumber } from "../../../../contracts/generated/doctor-contracts";
import { canonicalValueText, decimalSign, divergingBar, intervalContainsZero, matchKeyParts, orderQueryDeltas, ratioPercentFullText, ratioPercentText, signedValueText } from "./compare-display";

const int = (value: string): CanonicalNumber => ({ kind: "integer", value, scale: null });
const dec = (value: string): CanonicalNumber => ({ kind: "decimal", value, scale: value.split(".")[1]?.length ?? 0 });
const interval = (lower: string, upper: string, lowerInclusive: boolean, upperInclusive: boolean): BoundedInterval => ({ lower: int(lower), upper: int(upper), lower_inclusive: lowerInclusive, upper_inclusive: upperInclusive });

describe("compare display helpers", () => {
  it("reads signs from digits only", () => {
    expect(decimalSign("-0.000")).toBe(0);
    expect(decimalSign("-0.001")).toBe(-1);
    expect(decimalSign("12")).toBe(1);
  });

  it.each([
    [interval("-1000", "1000", false, false), true],
    [interval("0", "1000", true, false), true],
    [interval("0", "1000", false, true), false],
    [interval("-5", "0", false, false), false],
    [interval("-5", "0", false, true), true],
    [interval("3", "9", true, true), false],
  ])("decides whether an interval admits zero by endpoint sign and closure (%#)", (bounds, expected) => {
    expect(intervalContainsZero(bounds)).toBe(expected);
  });

  it("formats bytes, durations and counts adaptively, including fractional text-Profile bytes", () => {
    expect(canonicalValueText(int("0"), "byte")).toBe("0 B");
    expect(canonicalValueText(int("1"), "byte")).toBe("1 B");
    expect(canonicalValueText(int("128450000"), "byte")).toBe("122.499 MB");
    // "8.256 MB" in a text Profile is 8,657,043.456 bytes: it reads back as the same 8.256 MB.
    expect(canonicalValueText(dec("8657043.456"), "byte")).toBe("8.256 MB");
    expect(canonicalValueText(int("619380"), "ns")).toBe("619.38 μs");
    expect(canonicalValueText(int("1234567"), "item")).toBe("1,234,567");
    expect(signedValueText(int("-550000000"), "ns")).toBe("−550 ms");
    expect(signedValueText(int("2000"), "byte")).toBe("+1.953 KB");
    expect(signedValueText(int("0"), "byte")).toBe("0 B");
  });

  it("renders the backend ratio as a percent by moving the decimal point", () => {
    expect(ratioPercentText(int("1"))).toBe("+100%");
    expect(ratioPercentText(int("0"))).toBe("0%");
    expect(ratioPercentText(dec("-0.662000000"))).toBe("−66.2%");
    expect(ratioPercentText(dec("0.333333333"))).toBe("+33.3%");
    expect(ratioPercentText(dec("0.000049999"))).toBe("< +0.1%");
    expect(ratioPercentText(dec("-0.000016562"))).toBe("> −0.1%");
    expect(ratioPercentFullText(dec("-0.000016562"))).toBe("−0.0016562%");
    expect(ratioPercentFullText(dec("-0.8"))).toBe("−80%");
  });

  it("draws the bar from the ratio only and clamps beyond ±100%", () => {
    expect(divergingBar(null)).toBeNull();
    expect(divergingBar(dec("-0.5"))).toEqual({ left: 25, width: 25, clamped: false, direction: "down" });
    expect(divergingBar(dec("3.5"))).toEqual({ left: 50, width: 50, clamped: true, direction: "up" });
  });

  it("splits a match key at its first slash and orders query metrics", () => {
    expect(matchKeyParts("1:2:SELECT:25:-/__MAX_OF_OperatorTotalTime")).toEqual({ scope: "1:2:SELECT:25:-", counter: "__MAX_OF_OperatorTotalTime" });
    expect(matchKeyParts(undefined)).toBeNull();
    const ordered = orderQueryDeltas([{ metric_id: "Zeta" }, { metric_id: "QuerySpillBytes" }, { metric_id: "QueryTotalTime" }, { metric_id: "Alpha" }]);
    expect(ordered.map((item) => item.metric_id)).toEqual(["QueryTotalTime", "QuerySpillBytes", "Zeta", "Alpha"]);
  });
});
