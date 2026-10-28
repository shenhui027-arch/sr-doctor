import type { AuditCoverage, AuditErrorProportion, AuditMetricFact } from "../../../../contracts/generated/doctor-contracts";
import { bytesText, countText, durationNanosecondsText } from "../../lib/duration-display";

/** Render the server's exact fraction; never infer a percent from counters. */
export function errorProportionText(value: AuditErrorProportion): string {
  if (value.state === "exact") return `${value.numerator} / ${value.denominator}`;
  if (value.state === "not_applicable") return `— (not_applicable · ${value.numerator} / ${value.denominator})`;
  return `— (${value.state} · ${value.reason})`;
}

/** Display only: decimal shifting of an already normalized integer, never a
 * second SQL/Profile metric parser. Four significant digits, half-up rounding.
 * The source integer never passes through an IEEE-754 Number. */
export function auditScientific(value: string, decimalShift: number): string {
  const negative = value.startsWith("-");
  const digits = value.replace(/^-/, "").replace(/^0+/, "");
  if (digits === "") return "0.000e+0";
  let exponent = digits.length - 1 - decimalShift;
  let mantissa = BigInt(digits.slice(0, 4).padEnd(4, "0"));
  if (digits.length > 4 && digits[4] >= "5") mantissa += 1n;
  if (mantissa === 10000n) { mantissa = 1000n; exponent += 1; }
  const text = mantissa.toString().padStart(4, "0");
  return `${negative ? "−" : ""}${text[0]}.${text.slice(1)}e${exponent >= 0 ? "+" : ""}${exponent}`;
}

export function metricRawText(value: AuditMetricFact | null): string {
  if (!value) return "—";
  if (value.canonical_number && value.canonical_unit) {
    const canonical = `${value.canonical_number.value} ${value.canonical_unit}`;
    return value.state === "exact" ? canonical : `${canonical} (${value.state})`;
  }
  if (value.input.state === "text") return `${value.input.value} (${value.state})`;
  return `— (${value.state})`;
}

export function metricText(value: AuditMetricFact | null): string {
  const number = value?.canonical_number;
  const unit = value?.canonical_unit;
  if (!value || !number || number.kind !== "integer" || number.scale !== null || !/^-?\d+$/.test(number.value)) return metricRawText(value);
  if (unit !== "ns" && unit !== "byte" && unit !== "item") return metricRawText(value);
  const result = unit === "ns" ? durationNanosecondsText(number.value)! : unit === "byte" ? bytesText(number.value)! : countText(number.value)!;
  return value.state === "exact" ? result : `${result} (${value.state})`;
}

/** Display tone only (Tokens board): an exact fact renders as a plain number;
 * a server-qualified number (e.g. precision_unsafe) and a fact without a
 * canonical number (missing, invalid, sentinel, overflow, unavailable) render
 * as dashed tags. The text itself always comes from metricText, never 0. */
export function metricTone(value: AuditMetricFact | null): "exact" | "qualified" | "missing" {
  if (value?.state === "exact" && value.canonical_number) return "exact";
  return value?.canonical_number ? "qualified" : "missing";
}

export function auditMillisecondsText(value: string): string {
  return /^\d+$/.test(value) ? durationNanosecondsText((BigInt(value) * 1_000_000n).toString())! : `${value} ms`;
}

/** The server's reader-zone epoch coordinate is displayed as a clock, not as
 * an elapsed duration. Out-of-Date-range/unsafe values retain exact raw text. */
export function auditClockText(value: string): string {
  const milliseconds = Number(value);
  if (!/^\d+$/.test(value) || !Number.isSafeInteger(milliseconds) || milliseconds > 8_640_000_000_000_000) return `${value} ms`;
  return new Date(milliseconds).toISOString().replace("T", " ").replace("Z", "");
}

/** Coverage counters are canonical decimal strings; "0" is a real zero, not a warning. */
export function positiveCount(value: string | null | undefined): boolean {
  return typeof value === "string" && /^\d+$/.test(value) && !/^0+$/.test(value);
}

/**
 * Every retained event lacks a native digest: the server returned the
 * window, but `enable_sql_digest` was off, so no SQL family can exist.
 */
export function auditDigestUnavailable(coverage: Pick<AuditCoverage, "retained_event_count" | "missing_digest_count"> | null | undefined): boolean {
  return !!coverage && positiveCount(coverage.retained_event_count) && coverage.missing_digest_count === coverage.retained_event_count;
}
