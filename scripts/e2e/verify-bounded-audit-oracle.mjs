// Read-only, offline oracle for captured fixed-window Docker audit facts.
// Never connects, executes SQL, starts a service, or derives counts from Top-K.
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { join, resolve } from 'node:path';

const directory = resolve(process.argv[2] ?? '');
assert.ok(process.argv[2], 'An explicit capture directory is required');
const read = name => readFileSync(join(directory, name), 'utf8').replace(/^\uFEFF/, '').trim();
const hash = text => createHash('sha256').update(text).digest('hex');
const from = 1788591600000n, to = 1788595200000n, at = 1788592562000n;
let assertions = 0;
const equal = (actual, expected, label) => { assert.deepEqual(actual, expected, label); assertions++; };
function table(name) {
  const [header, ...lines] = read(name).split(/\r?\n/);
  const keys = header.split('\t');
  return lines.map(line => {
    const values = line.split('\t');
    assert.equal(values.length, keys.length, `${name}: malformed TSV`);
    return Object.fromEntries(keys.map((key, index) => [key, values[index] === 'NULL' ? null : values[index]]));
  });
}
const before = read('source-before.tsv'), after = read('source-after.tsv');
equal(before, after, 'bounded source changed during non-atomic reads');
const [header, ...lines] = before.split(/\r?\n/);
equal(header, 'source_row', 'raw source projection');
assert.ok(lines.length < 501, 'Oracle source cap reached; not a full cohort');
const rows = lines.map(line => JSON.parse(line));
for (const row of rows) {
  assert.equal(row.length, 14);
  assert.ok(row.every(cell => cell === null || typeof cell === 'string'), 'Lossless source text required');
  const start = BigInt(row[13]) * 1000n;
  assert.ok(start >= from - 3600000n && start < to, 'source escaped fixed bounds');
}
const idPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
function canonical(source) {
  const groups = new Map();
  for (const row of source.filter(row => idPattern.test(row[0] ?? ''))) {
    const group = groups.get(row[0]) ?? []; group.push(row); groups.set(row[0], group);
  }
  const retained = [], conflicts = [], duplicates = [];
  for (const group of groups.values()) {
    if (group.every(row => JSON.stringify(row) === JSON.stringify(group[0]))) {
      retained.push(group[0]); duplicates.push(...group.slice(1));
    } else conflicts.push(...group);
  }
  return { retained, conflicts, duplicates };
}
const windowRows = rows.filter(row => BigInt(row[13]) * 1000n >= from);
const normal = canonical(windowRows), historical = canonical(rows);
const [coverage] = table('coverage.tsv');
const count = (key, value) => equal(coverage[key], String(value), key);
const validContext = row => [9, 10].every(index => row[index] !== null && row[index] !== '' && Buffer.byteLength(row[index]) <= 256);
const validDigest = row => /^[0-9a-f]{1,128}$/i.test(row[12] ?? '');
count('raw_row_count', windowRows.length);
count('invalid_query_id_count', windowRows.filter(row => !idPattern.test(row[0] ?? '')).length);
count('exact_duplicate_row_count', normal.duplicates.length);
count('conflicting_duplicate_row_count', normal.conflicts.length);
count('retained_event_count', normal.retained.length);
count('missing_digest_count', normal.retained.filter(row => !validDigest(row)).length);
count('missing_context_count', normal.retained.filter(row => !validContext(row)).length);
count('retained_family_count', new Set(normal.retained.filter(row => validContext(row) && validDigest(row)).map(row => JSON.stringify([row[9], row[10], row[12]]))).size);
count('error_count', normal.retained.filter(row => row[2] === 'ERR').length);
for (const [prefix, index, aggregate, mode] of [
  ['query_time', 3, 'cumulative_query_time', 'sum'], ['scan_bytes', 4, 'cumulative_scan_bytes', 'sum'],
  ['cpu_cost', 6, 'cumulative_cpu_cost', 'sum'], ['memory_cost', 7, 'max_memory_cost', 'max'],
]) {
  const present = normal.retained.map(row => row[index]).filter(cell => cell !== null).map(BigInt);
  const nonnegative = present.filter(value => value >= 0n);
  count(`${prefix}_sql_null_count`, normal.retained.length - present.length);
  count(`${prefix}_text_present_count`, present.length);
  count(`${prefix}_nonnegative_count`, nonnegative.length);
  count(`${prefix}_negative_count`, present.length - nonnegative.length);
  const result = nonnegative.length === 0 ? null : String(mode === 'sum' ? nonnegative.reduce((a, b) => a + b, 0n) : nonnegative.reduce((a, b) => a > b ? a : b));
  equal(coverage[aggregate], result, aggregate);
}
function exclusion(row) {
  if (row[1] === null) return 'timestamp_missing';
  if (row[13] === null) return 'timestamp_invalid';
  if (!['EOF', 'ERR'].includes(row[2])) return 'non_terminal';
  if (row[3] === null) return 'query_time_sql_null';
  const duration = BigInt(row[3]);
  if (duration < 0n) return 'query_time_negative';
  if (duration > 9223372036854n) return 'query_time_overflow';
  if (duration === 0n) return 'zero_duration';
  return null;
}
const valid = historical.retained.filter(row => exclusion(row) === null).map(row => ({ row, start: BigInt(row[13]) * 1000n, end: BigInt(row[13]) * 1000n + BigInt(row[3]) }));
const overlap = valid.filter(event => event.start < to && event.end > from);
const deltas = new Map();
for (const event of overlap) for (const [point, delta] of [[event.start, 1n], [event.end, -1n]]) {
  if (point > from && point < to) deltas.set(point, (deltas.get(point) ?? 0n) + delta);
}
const transitions = [...deltas].filter(([, delta]) => delta !== 0n).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0);
assert.ok(transitions.length <= 2047, 'This capture must exercise the exact-series path');
const curve = table('concurrency.tsv'), summary = curve[0];
equal(summary.row_kind, '0', 'curve header row');
equal(summary.canonical_bounded_event_count, String(historical.retained.length), 'historical cohort');
equal(summary.interval_count, String(overlap.length), 'overlap cohort');
equal(summary.initial_active_count, String(overlap.filter(event => event.start <= from && event.end > from).length), 'left boundary count');
equal(summary.transition_count, String(transitions.length), 'transition count');
for (const reason of ['timestamp_missing', 'timestamp_invalid', 'non_terminal', 'query_time_sql_null', 'query_time_negative', 'query_time_overflow', 'zero_duration']) {
  equal(summary[`${reason}_count`], String(historical.retained.filter(row => exclusion(row) === reason).length), reason);
}
equal(summary.endpoint_overflow_count, '0', 'bounded LARGEINT endpoints');
equal(curve.slice(1).map(row => [row.row_kind, row.at_ms, row.delta]), transitions.map(([point, delta]) => ['1', String(point), String(delta)]), 'every exact transition');
const active = table('active-at.tsv'), expected = valid.filter(event => event.start <= at && event.end > at).sort((a, b) => a.row[0].localeCompare(b.row[0]));
assert.ok(expected.length <= 500, 'Active-at capture must not be truncated');
equal(active[0].active_count, String(expected.length), 'active count');
equal(active.slice(1).map(row => row.query_id), expected.map(event => event.row[0]), 'actual participant IDs, not Top-K');
equal(active.slice(1).map(row => [row.event_epoch_seconds, row.query_time, row.state]), expected.map(event => [event.row[13], event.row[3], event.row[2]]), 'participant interval facts');
const receipt = { status: 'PASS', scope: 'fixed_2026-09-05_07:00_to_08:00_UTC', source_window: '06:00_to_08:00_UTC', assertions, raw_rows: rows.length, retained_events: normal.retained.length, transitions: transitions.length, active_at_participants: expected.length, source_sha256: hash(before), source_unchanged_between_reads: true, real_database_sql: true, http_api_tested: false, candidate_tested: false };
writeFileSync(join(directory, 'oracle-receipt.json'), JSON.stringify(receipt, null, 2) + '\n');
console.log(JSON.stringify(receipt, null, 2));
