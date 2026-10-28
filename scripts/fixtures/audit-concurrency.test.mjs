import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { fileURLToPath } from "node:url";

const FIXTURE_PATH = fileURLToPath(new URL("../../fixtures/synthetic/audit-concurrency-v1.json", import.meta.url));
const fixture = JSON.parse(readFileSync(FIXTURE_PATH, "utf8"));
const SAFE_INTEGER_LIMIT = 9007199254740991n;

function decimal(value, label) {
  assert.equal(typeof value, "string", `${label} must remain a decimal string`);
  assert.match(value, /^(0|[1-9][0-9]*)$/, `${label} must be a non-negative decimal string`);
  return BigInt(value);
}

// Test-only interval reference. This is not a product algorithm or a graph receipt fixture.
function knownIntervals(group) {
  return group.events.flatMap((event) => {
    if (event.state !== "available") return [];
    const start = decimal(event.recorded_start_ms, `${group.group_id}.${event.id}.recorded_start_ms`);
    const duration = decimal(event.duration_ms, `${group.group_id}.${event.id}.duration_ms`);
    const end = start + duration;
    if (duration === 0n) return [];
    return [{ id: event.id, start, end }];
  });
}

function expectedIntervals(group) {
  const windowStart = decimal(group.window.start_ms, `${group.group_id}.window.start_ms`);
  const windowEnd = decimal(group.window.end_ms, `${group.group_id}.window.end_ms`);
  assert.ok(windowStart < windowEnd, `${group.group_id} window must be increasing`);
  return { windowStart, windowEnd, intervals: knownIntervals(group) };
}

function overlapIds(group) {
  const { windowStart, windowEnd, intervals } = expectedIntervals(group);
  return intervals
    .filter(({ start, end }) => start < windowEnd && end > windowStart)
    .map(({ id }) => id);
}

function activeAt(group, pointText) {
  const point = decimal(pointText, `${group.group_id}.active_at.${pointText}`);
  return knownIntervals(group)
    .filter(({ start, end }) => start <= point && end > point)
    .map(({ id }) => id);
}

function decimalCount(value, label) {
  const count = decimal(value, label);
  assert.ok(count >= 0n, `${label} must not be negative`);
  return count;
}

function countBy(events, field, predicate) {
  const counts = {};
  for (const event of events) {
    if (!predicate(event)) continue;
    const key = event[field];
    counts[key] = (counts[key] ?? 0n) + 1n;
  }
  return Object.fromEntries(Object.entries(counts).map(([key, count]) => [key, String(count)]));
}

function observedCoverage(group) {
  const available = group.events.filter((event) => event.state === "available");
  const zeroDuration = available.filter((event) => decimal(event.duration_ms, `${group.group_id}.${event.id}.duration_ms`) === 0n);
  const unknown = group.events.filter((event) => event.state === "unfinished" || event.state === "not_ingested");
  const typedExcluded = group.events.filter((event) => event.state !== "available" && !unknown.includes(event));
  return {
    observed_records: String(group.events.length),
    available_records: String(available.length),
    known_nonzero_intervals: String(available.length - zeroDuration.length),
    zero_duration_records: String(zeroDuration.length),
    overlap_records: String(overlapIds(group).length),
    unknown_records: String(unknown.length),
    typed_excluded_records: String(typedExcluded.length),
    typed_excluded_by_reason: countBy(typedExcluded, "exclusion_reason", () => true),
    unknown_by_reason: countBy(unknown, "unknown_reason", () => true),
  };
}

function assertManualCoverage(group) {
  const expected = group.expected;
  const observed = observedCoverage(group);
  for (const [key, value] of Object.entries(expected.counts)) {
    assert.equal(decimalCount(value, `${group.group_id}.expected.counts.${key}`), decimalCount(observed[key], `${group.group_id}.observed.${key}`), `${group.group_id} ${key}`);
  }
  assert.deepEqual(observed.typed_excluded_by_reason, expected.coverage.typed_excluded_by_reason);
  assert.deepEqual(observed.unknown_by_reason, expected.coverage.unknown_by_reason);
  assert.equal(BigInt(expected.coverage.unknown_tail_ids.length), decimalCount(expected.counts.unknown_records, `${group.group_id}.unknown_records`));
}

function assertManualEndpoints(group) {
  for (const event of group.events.filter((candidate) => candidate.state === "available")) {
    const start = decimal(event.recorded_start_ms, `${group.group_id}.${event.id}.recorded_start_ms`);
    const duration = decimal(event.duration_ms, `${group.group_id}.${event.id}.duration_ms`);
    const expectedEnd = decimal(group.expected.end_ms[event.id], `${group.group_id}.expected.end_ms.${event.id}`);
    assert.equal(start + duration, expectedEnd, `${group.group_id}.${event.id} end endpoint`);
  }
}

test("fixture exposes an explicit synthetic half-open contract", () => {
  assert.equal(fixture.source_is_synthetic, true);
  assert.equal(fixture.fixture_id, "audit-concurrency-v1");
  assert.equal(fixture.contract.interval, "[T,T+D)");
  assert.equal(fixture.contract.overlap, "T < window_end_ms && T+D > window_start_ms");
  assert.equal(fixture.contract.active_at, "T <= point_ms && T+D > point_ms");
  assert.match(fixture.contract.zero_duration, /explicitly excluded/);
  assert.match(fixture.contract.unknown_tail, /must not be filled/);
  assert.equal(fixture.groups.length, 4);
  for (const group of fixture.groups) assert.equal(group.source_is_synthetic, true, group.group_id);
});

test("manual endpoints, overlap sets, and active-at sets use exact BigInt semantics", () => {
  for (const group of fixture.groups) {
    assertManualEndpoints(group);
    assert.deepEqual(overlapIds(group), group.expected.overlap_ids, `${group.group_id} overlap set`);
    for (const [point, expectedIds] of Object.entries(group.expected.active_at)) {
      assert.deepEqual(activeAt(group, point), expectedIds, `${group.group_id} active at ${point}`);
    }
    assertManualCoverage(group);
  }
});

test("cross-window sample includes a query started before the window and exact same-time endpoint behavior", () => {
  const group = fixture.groups.find(({ group_id }) => group_id === "cross-window-endpoint-zero");
  assert.ok(group);
  assert.ok(group.expected.active_at["1000"].includes("long-before-window"));
  assert.deepEqual(activeAt(group, "3500"), ["long-before-window", "starts-at-3500"]);
  assert.deepEqual(activeAt(group, "5000"), ["long-before-window", "touches-window-right"]);
  assert.ok(!group.expected.overlap_ids.includes("touches-window-left"));
  assert.ok(!group.expected.overlap_ids.includes("touches-window-right"));
  assert.ok(!group.expected.overlap_ids.includes("query-time-zero"));
});

test("latest tail keeps unfinished and not-ingested state unknown instead of filling real zero", () => {
  const group = fixture.groups.find(({ group_id }) => group_id === "latest-tail-unknown");
  assert.ok(group);
  assert.equal(group.expected.coverage.unknown_is_not_zero, true);
  assert.deepEqual(group.expected.coverage.unknown_tail_ids, ["latest-unfinished", "latest-not-ingested"]);
  assert.equal(decimalCount(group.expected.counts.unknown_records, "latest-tail unknown_records"), 2n);
  for (const ids of Object.values(group.expected.active_at)) {
    assert.ok(!ids.some((id) => group.expected.coverage.unknown_tail_ids.includes(id)));
  }
  assert.deepEqual(activeAt(group, "9900"), []);
});

test("typed exclusion coverage is independent and valid endpoints exceed JavaScript safe integer range", () => {
  const group = fixture.groups.find(({ group_id }) => group_id === "typed-exclusion-bigint");
  assert.ok(group);
  const firstStart = decimal(group.events[0].recorded_start_ms, "valid-bigint.recorded_start_ms");
  assert.ok(firstStart > SAFE_INTEGER_LIMIT);
  assert.deepEqual(Object.keys(group.expected.coverage.typed_excluded_by_reason).sort(), [
    "conflicting_record",
    "duplicate_record",
    "invalid_start",
    "missing_duration",
    "negative_duration",
    "overflow_duration",
  ]);
  assert.equal(decimalCount(group.expected.counts.typed_excluded_records, "typed exclusion count"), 6n);
  assert.deepEqual(overlapIds(group), ["valid-bigint", "valid-bigint-next"]);
  assert.deepEqual(activeAt(group, "9007199254740996"), ["valid-bigint-next"]);
  assert.deepEqual(activeAt(group, "9007199254740997"), []);
});

test("Figma-aligned mock keeps started-in-window and overlap populations distinct", () => {
  const group = fixture.groups.find(({ group_id }) => group_id === "figma-window-population-mock");
  assert.ok(group);
  assert.equal(group.is_mock, true);
  assert.match(group.metric_source, /synthetic-only; not real database data/);
  const windowStart = decimal(group.window.start_ms, `${group.group_id}.window.start_ms`);
  const windowEnd = decimal(group.window.end_ms, `${group.group_id}.window.end_ms`);
  const startedInWindow = group.events.filter((event) => {
    const start = decimal(event.recorded_start_ms, `${group.group_id}.${event.id}.recorded_start_ms`);
    return start >= windowStart && start < windowEnd;
  });
  assert.deepEqual(startedInWindow.map(({ id }) => id), group.expected.window_started_ids);
  assert.equal(Object.hasOwn(group.events.find(({ id }) => id === "A"), "scan_bytes"), false);
  const scanBytesRecords = startedInWindow.filter(({ scan_bytes }) => typeof scan_bytes === "string");
  const scanBytes = scanBytesRecords.reduce(
    (sum, event) => sum + decimal(event.scan_bytes, `${group.group_id}.${event.id}.scan_bytes`),
    0n,
  );
  assert.equal(scanBytes, decimal(group.expected.window_started_scan_bytes, "window-started scan bytes"));
  assert.equal(
    BigInt(startedInWindow.length),
    decimal(group.expected.window_started_record_count, "window-started record count"),
  );
  assert.equal(
    BigInt(scanBytesRecords.length),
    decimal(group.expected.window_started_scan_bytes_valid_records, "window-started valid scan records"),
  );
  assert.equal(
    BigInt(startedInWindow.filter(({ terminal_state }) => terminal_state === "ERR").length),
    decimal(group.expected.window_started_error_records, "window-started error records"),
  );
  assert.equal(decimalCount(group.expected.window_started_duration_ms, "window-started duration"), 5880000n);
  assert.deepEqual(group.expected.overlap_population_ids, group.expected.overlap_ids);
  assert.deepEqual(group.expected.outside_window_started_not_in_cards_or_family_top, ["A"]);
  assert.deepEqual(activeAt(group, "3120000"), ["A", "B", "C", "D"]);
  assert.deepEqual(group.expected.family_examples.map(({ execution_count }) => execution_count), ["3", "2", "2", "1"]);
  assert.equal(group.expected.family_examples.reduce((sum, family) => sum + decimal(family.execution_count, `${family.family_id}.execution_count`), 0n), 8n);
  assert.equal(group.expected.family_examples.reduce((sum, family) => sum + decimal(family.cumulative_duration_ms, `${family.family_id}.cumulative_duration_ms`), 0n), 5880000n);
});
