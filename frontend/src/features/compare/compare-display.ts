import type { BoundedInterval, CanonicalNumber, MetricCell } from "../../../../contracts/generated/doctor-contracts";
import { bytesText, countText, durationNanosecondsText } from "../../lib/duration-display";

/**
 * Display-only formatting of comparator DTO numbers. Nothing here recomputes a
 * delta, a percentage or a comparability decision; every function reads one
 * backend value and renders it.
 */

/** Sign of a base-10 string, read from its characters (no numeric conversion). */
export function decimalSign(value: string): -1 | 0 | 1 {
  if (!/[1-9]/.test(value)) return 0;
  return value.trim().startsWith("-") ? -1 : 1;
}

/** True when the DTO interval admits 0, decided from endpoint signs and closure only. */
export function intervalContainsZero(bounds: BoundedInterval): boolean {
  const lower = decimalSign(bounds.lower.value);
  const upper = decimalSign(bounds.upper.value);
  const lowerAdmits = lower < 0 || (lower === 0 && bounds.lower_inclusive);
  const upperAdmits = upper > 0 || (upper === 0 && bounds.upper_inclusive);
  return lowerAdmits && upperAdmits;
}

/**
 * The endpoint nearest 0 of an interval that excludes 0: the smallest change the
 * DTO proves, used for direction and the bar. Null when the interval admits 0.
 */
export function provenBound(bounds: BoundedInterval): CanonicalNumber | null {
  if (intervalContainsZero(bounds)) return null;
  return decimalSign(bounds.lower.value) >= 0 ? bounds.lower : bounds.upper;
}

export function exactUnit(cell: MetricCell | undefined): string | null {
  return cell?.state === "exact" ? cell.canonical_unit : null;
}

/** One canonical value in its adaptive display unit: bytes, durations and counts. */
export function canonicalValueText(number: CanonicalNumber, unit: string | null): string {
  // Text Profiles normalise "8.256 MB" to fractional bytes; whole bytes/ns are the display floor.
  const integer = number.kind === "integer" && number.scale === null ? number.value : roundedInteger(number.value);
  if (integer !== null && (number.kind === "integer" || unit === "ns" || unit === "byte" || unit === "bytes")) {
    if (unit === "ns") { const text = durationNanosecondsText(integer); if (text) return text; }
    if (unit === "byte" || unit === "bytes") { const text = bytesText(integer); if (text) return text; }
    if (unit === "item" || unit === "count" || unit === "rows" || unit === null) { const text = countText(integer); if (text) return text; }
  }
  const raw = number.value.replace(/^-/, "−");
  return unit && unit !== "item" && unit !== "count" ? `${raw} ${unit}` : raw;
}

/** Half-up rounding of a base-10 decimal string to a whole number, on the digits only. */
function roundedInteger(value: string): string | null {
  const match = /^(-?)(\d+)(?:\.(\d*))?$/.exec(value.trim());
  if (!match) return null;
  const [, sign, whole, fraction = ""] = match;
  const magnitude = BigInt(whole) + ((fraction[0] ?? "0") >= "5" ? 1n : 0n);
  return magnitude === 0n ? "0" : `${sign}${magnitude}`;
}

/** A delta value with an explicit sign; zero stays unsigned. */
export function signedValueText(number: CanonicalNumber, unit: string | null): string {
  const text = canonicalValueText(number, unit);
  return decimalSign(number.value) > 0 ? `+${text}` : text;
}

function bracket(bounds: BoundedInterval, render: (number: CanonicalNumber) => string): string {
  return `${bounds.lower_inclusive ? "[" : "("}${render(bounds.lower)}, ${render(bounds.upper)}${bounds.upper_inclusive ? "]" : ")"}`;
}

export function intervalText(bounds: BoundedInterval, unit: string | null): string {
  return bracket(bounds, (number) => canonicalValueText(number, unit));
}

/**
 * The comparator's `percentage_value` / `percentage_bounds` are ratios
 * (`absolute_delta / baseline`, fixed scale 9; see
 * `contracts/manifests/doctor-comparator-v0.1.json`). Rendering as a percent
 * only moves the decimal point two places on the string.
 */
export function ratioPercentFullText(number: CanonicalNumber): string {
  const { negative, whole, fraction } = shiftedPercent(number.value);
  const trimmed = fraction.replace(/0+$/, "");
  const sign = negative && /[1-9]/.test(whole + fraction) ? "−" : "";
  return `${sign}${whole}${trimmed ? `.${trimmed}` : ""}%`;
}

/** Percent with at most one decimal (half-up, display only), signed; the full value is `ratioPercentFullText`. */
export function ratioPercentText(number: CanonicalNumber): string {
  const { negative, whole, fraction } = shiftedPercent(number.value);
  let tenths = BigInt(`${whole}${fraction.padEnd(1, "0").slice(0, 1)}`);
  if ((fraction[1] ?? "0") >= "5") tenths += 1n;
  const nonZero = /[1-9]/.test(whole + fraction);
  if (tenths === 0n) return nonZero ? (negative ? "> −0.1%" : "< +0.1%") : "0%";
  const tenth = tenths % 10n;
  const text = `${tenths / 10n}${tenth ? `.${tenth}` : ""}%`;
  return `${negative ? "−" : "+"}${text}`;
}

export function ratioIntervalText(bounds: BoundedInterval): string {
  return bracket(bounds, ratioPercentFullText);
}

function shiftedPercent(value: string): { negative: boolean; whole: string; fraction: string } {
  const trimmed = value.trim();
  const negative = trimmed.startsWith("-");
  const body = trimmed.replace(/^[-+]/, "");
  const [integerPart = "0", fractionPart = ""] = body.split(".");
  const padded = fractionPart.padEnd(2, "0");
  const whole = `${integerPart}${padded.slice(0, 2)}`.replace(/^0+(?=\d)/, "");
  return { negative, whole, fraction: padded.slice(2) };
}

export type DivergingBar = { left: number; width: number; clamped: boolean; direction: "up" | "down" | "none" };

/** Bar geometry from the backend ratio only: half the track equals ±100%, larger values clamp visually. */
export function divergingBar(ratio: CanonicalNumber | null): DivergingBar | null {
  if (!ratio) return null;
  const value = Number(ratio.value);
  if (!Number.isFinite(value)) return null;
  const magnitude = Math.min(Math.abs(value), 1);
  const width = magnitude * 50;
  const direction = value < 0 ? "down" : value > 0 ? "up" : "none";
  return { left: value < 0 ? 50 - width : 50, width, clamped: Math.abs(value) > 1, direction };
}

/** Statistic hint of a counter: the raw counter name after the structural key's `/`. */
export function matchKeyParts(matchKey: string | undefined): { scope: string; counter: string } | null {
  if (!matchKey) return null;
  const slash = matchKey.indexOf("/");
  if (slash < 0) return { scope: "", counter: matchKey };
  return { scope: matchKey.slice(0, slash), counter: matchKey.slice(slash + 1) };
}

/** Query-level metrics shown first, in this order; any other query metric follows in DTO order. */
export const QUERY_METRIC_ORDER = [
  "QueryTotalTime",
  "QueryExecutionWallTime",
  "QueryCumulativeCpuTime",
  "QueryCumulativeScanTime",
  "QueryPeakMemoryUsagePerNode",
  "QuerySpillBytes",
  "QueryCumulativeNetworkTime",
  "QueryCumulativeOperatorTime",
] as const;

export function orderQueryDeltas<T extends { metric_id: string }>(deltas: T[]): T[] {
  const rank = (id: string) => {
    const index = (QUERY_METRIC_ORDER as readonly string[]).indexOf(id);
    return index < 0 ? QUERY_METRIC_ORDER.length : index;
  };
  return deltas.map((delta, index) => ({ delta, index }))
    .sort((left, right) => rank(left.delta.metric_id) - rank(right.delta.metric_id) || left.index - right.index)
    .map(({ delta }) => delta);
}
