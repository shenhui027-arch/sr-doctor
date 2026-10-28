import { describe, expect, it } from "vitest";
import vectors from "../../../fixtures/expected/display-units.json";
import { bytesText, compactCountText, countText, durationNanosecondsText } from "./duration-display";

describe("adaptive normalized duration", () => {
  it.each([
    ["0", "0 ns"], ["1", "1 ns"], ["999", "999 ns"], ["1000", "1 μs"],
    ["1001", "1.001 μs"], ["999999", "999.999 μs"], ["1000000", "1 ms"],
    ["1000499", "1 ms"], ["1000500", "1.001 ms"],
    ["999999499", "999.999 ms"], ["999999500", "1 s"],
    ["1000000000", "1 s"], ["1000499999", "1 s"], ["1000500000", "1.001 s"],
    ["619380", "619.38 μs"], ["29545", "29.545 μs"], ["1206000", "1.206 ms"], ["31880", "31.88 μs"],
    ["-1000500", "−1.001 ms"], ["9007199254740993", "2502h 00m"],
    ["9223372036854775807", "2562047h 47m"],
    ["170141183460469231731687303715884105727", "47261439850130342147690917h 42m"],
    ["-170141183460469231731687303715884105728", "−47261439850130342147690917h 42m"],
  ])("formats %s without floating-point conversion", (value, expected) => {
    expect(durationNanosecondsText(value)).toBe(expected);
  });

  it.each(["", "unknown", "1ms", "1.5", "1e9", " 1", "+1"])("does not parse raw metric %s", value => {
    expect(durationNanosecondsText(value)).toBeNull();
  });
});

describe("shared display-unit vectors (v0.6-09)", () => {
  it("matches fixtures/expected/display-units.json, the same vectors the backend display module checks", () => {
    expect(vectors.schema_version).toBe("sr-doctor.display-units/v1");
    expect(vectors.cases.length).toBeGreaterThanOrEqual(20);
    const format: Record<string, (value: string) => string | null> = { duration_ns: durationNanosecondsText, bytes: bytesText, count: countText, count_compact: compactCountText };
    for (const testCase of vectors.cases) expect(format[testCase.kind](testCase.value), JSON.stringify(testCase)).toBe(testCase.text);
  });
});
