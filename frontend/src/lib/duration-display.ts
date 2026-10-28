/** Display a normalized integer duration, not raw SQL/Profile metric text.
 * Matches the report projection: at most three decimals, half-up, then unit carry.
 * BigInt preserves canonical precision above 2^53; callers retain typed states. */
export function durationNanosecondsText(value: string): string | null {
  if (!/^-?\d+$/.test(value)) return null;
  const integer = BigInt(value);
  const magnitude = integer < 0n ? -integer : integer;
  // Below this the three-decimal seconds text stays under "60 s"; above it,
  // minutes and seconds, then hours and minutes, as StarRocks prints them.
  if (magnitude < 59_999_500_000n) return adaptiveIntegerText(value, ["ns", "μs", "ms", "s"]);
  const sign = integer < 0n ? "−" : "";
  const second = 1_000_000_000n;
  const seconds = (magnitude + second / 2n) / second;
  if (seconds < 3600n) return `${sign}${seconds / 60n}m ${String(seconds % 60n).padStart(2, "0")}s`;
  const minutes = (magnitude + 30n * second) / (60n * second);
  return `${sign}${minutes / 60n}h ${String(minutes % 60n).padStart(2, "0")}m`;
}

/** Bytes with StarRocks' binary KB / MB / GB (1 MB = 1,048,576 bytes), as its Profile prints them; the backend's
 * `display::bytes` follows the same policy, and both are checked against fixtures/expected/display-units.json. */
export function bytesText(value: string): string | null {
  return adaptiveIntegerText(value, ["B", "KB", "MB", "GB", "TB", "PB", "EB"], 1024n);
}

/** Group a normalized integer count by thousands; counts carry no unit label. */
export function countText(value: string): string | null {
  if (!/^-?\d+$/.test(value)) return null;
  const negative = value.startsWith("-");
  const digits = value.replace(/^-/, "");
  return `${negative ? "−" : ""}${digits.replace(/\B(?=(\d{3})+(?!\d))/g, ",")}`;
}

/** Dense count for tables and cards (backend `display::count_compact`): grouped below 1,000,
 * then K with no decimals, M with one, B and T with two; half-up with carry. */
export function compactCountText(value: string): string | null {
  if (!/^-?\d+$/.test(value)) return null;
  const integer = BigInt(value);
  const magnitude = integer < 0n ? -integer : integer;
  if (magnitude < 1000n) return countText(value);
  const units = [["K", 1_000n, 0], ["M", 1_000_000n, 1], ["B", 1_000_000_000n, 2], ["T", 1_000_000_000_000n, 2]] as const;
  let index = units.length - 1;
  while (index > 0 && magnitude < units[index][1]) index -= 1;
  for (;;) {
    const [unit, factor, decimals] = units[index];
    const scale = 10n ** BigInt(decimals);
    const scaled = (magnitude * scale + factor / 2n) / factor;
    const whole = scaled / scale;
    if (whole >= 1000n && index + 1 < units.length) { index += 1; continue; }
    const fraction = decimals === 0 ? "" : `.${String(scaled % scale).padStart(decimals, "0")}`;
    return `${integer < 0n ? "−" : ""}${whole}${fraction} ${unit}`;
  }
}

function adaptiveIntegerText(value: string, labels: string[], base = 1000n): string | null {
  if (!/^-?\d+$/.test(value)) return null;
  const integer = BigInt(value);
  const magnitude = integer < 0n ? -integer : integer;
  const units = labels.map((label, index) => [label, base ** BigInt(index)] as const);
  let index = 0;
  while (index + 1 < units.length && magnitude >= units[index + 1][1]) index += 1;
  for (;;) {
    const [unit, factor] = units[index];
    const rounded = (magnitude * 1_000n + factor / 2n) / factor;
    const whole = rounded / 1_000n;
    const fraction = rounded % 1_000n;
    if (whole >= base && index + 1 < units.length) { index += 1; continue; }
    const decimal = fraction === 0n ? "" : `.${fraction.toString().padStart(3, "0")}`.replace(/0+$/, "");
    return `${integer < 0n ? "−" : ""}${whole}${decimal} ${unit}`;
  }
}
