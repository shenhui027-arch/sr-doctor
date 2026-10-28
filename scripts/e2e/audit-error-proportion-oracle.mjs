// Pure fixture/oracle: emits read-only SQL or verifies an explicit local capture.
// No database, HTTP, shell, service or user SQL execution occurs in this script.
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createHash } from 'node:crypto';

export const CAP = 3_000_000_000_000n;
const SCALE = 10n ** 25n;
export const cases = [
  ['bounded', 'a_error_count_high', 100n, 1000n],
  ['bounded', 'b_ratio_high', 2n, 4n],
  ['bounded', 'c_equal_ratio', 1n, 2n],
  ['bounded', 'd_zero_small', 0n, 1n],
  ['bounded', 'e_zero_cap', 0n, CAP],
  ['bounded', 'f_one_at_cap', CAP, CAP],
  ['bounded', 'g_close_lower', CAP - 2n, CAP - 1n],
  ['bounded', 'h_close_higher', CAP - 1n, CAP],
  ['collision', 'a_lower', 1n, 10_000_000_000_001n],
  ['collision', 'b_higher', 1n, 10_000_000_000_000n],
  ['overcap', 'a_overcap', CAP + 1n, CAP + 1n],
  ['overflow_cast', 'a_bigint_max', 9_223_372_036_854_775_807n, 9_223_372_036_854_775_807n],
];

export function oracleSql() {
  const source = cases.map(([population, id, e, n]) =>
    `SELECT '${population}' AS population, '${id}' AS family_id, CAST('${e}' AS BIGINT) AS error_count, CAST('${n}' AS BIGINT) AS canonical_execution_count`).join('\nUNION ALL\n');
  return `-- Synthetic aggregate populations; no audit table scan and no DDL/DML.\nWITH family_aggregates AS (\n${source}\n), family_rankable AS (\n SELECT *, MAX(canonical_execution_count) OVER (PARTITION BY population) AS population_max FROM family_aggregates\n)\nSELECT /*+ SET_VAR(query_timeout = 10) */ population, family_id, error_count, canonical_execution_count, population_max,\n CAST(LEAST(error_count, ${CAP}) AS DECIMAL(38,25)) / CAST(canonical_execution_count AS DECIMAL(38,0)) AS sort_key\nFROM family_rankable ORDER BY population ASC, sort_key DESC, family_id ASC;\n`;
}

function scaledDecimal(text) {
  assert.match(text, /^\d+(?:\.\d{1,25})?$/);
  const [whole, fraction = ''] = text.split('.');
  return BigInt(whole) * SCALE + BigInt(fraction.padEnd(25, '0'));
}

export function verifyOracle(capture) {
  const rows = capture.trim().split(/\r?\n/).map(line => line.split('\t'));
  const expected = new Map(cases.map(row => [row[1], row]));
  const populations = new Map();
  const checks = [];
  const check = (name, condition) => { assert.ok(condition, name); checks.push(name); };
  check('all_synthetic_rows_returned', rows.length === cases.length);
  const seen = new Set();
  for (const row of rows) {
    assert.equal(row.length, 6);
    const [population, id, e, n, max, key] = row;
    check(`${id}:unique`, !seen.has(id)); seen.add(id);
    const wanted = expected.get(id);
    assert.ok(wanted, 'unknown synthetic family');
    check(`${id}:source_values`, population === wanted[0] && e === String(wanted[2]) && n === String(wanted[3]));
    const populationMax = cases.filter(c => c[0] === population).reduce((a, c) => c[3] > a ? c[3] : a, 0n);
    check(`${id}:same_population_guard`, BigInt(max) === populationMax);
    const numerator = (wanted[2] > CAP ? CAP : wanted[2]) * SCALE;
    const quotient = numerator / wanted[3];
    const halfUp = quotient + (numerator % wanted[3] * 2n >= wanted[3] ? 1n : 0n);
    check(`${id}:decimal_half_up`, scaledDecimal(key) === halfUp);
    const group = populations.get(population) ?? []; group.push({ id, e: BigInt(e), n: BigInt(n), key }); populations.set(population, group);
  }
  const bounded = populations.get('bounded');
  const exact = [...bounded].sort((a, b) => {
    const cross = a.e * b.n - b.e * a.n;
    return cross > 0n ? -1 : cross < 0n ? 1 : a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
  });
  check('bounded_order_matches_independent_bigint_cross_products', bounded.every((row, i) => row.id === exact[i].id));
  check('ratio_order_differs_from_error_count_order', bounded.findIndex(r => r.id === 'b_ratio_high') < bounded.findIndex(r => r.id === 'a_error_count_high'));
  const collision = populations.get('collision');
  check('beyond_cap_keys_really_collide', scaledDecimal(collision[0].key) === scaledDecimal(collision[1].key));
  check('colliding_keys_are_not_equal_fractions', collision[0].e * collision[1].n !== collision[1].e * collision[0].n);
  check('cap_proof_uses_strict_grid_separation', CAP * CAP < SCALE);
  return { status: 'PASS', mode: 'synthetic_sql_numeric_oracle_not_http_or_audit_ingestion', checks, rows: rows.length,
    capture_sha256: createHash('sha256').update(capture).digest('hex'),
    sql_sha256: createHash('sha256').update(oracleSql()).digest('hex'),
    limitations: ['outside_cap_rows_are_diagnostic_only_never_publishable', 'connector_generation_guard_requires_separate_tests', 'not_a_real_audit_family_cohort'] };
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  if (process.argv[2] === '--sql') process.stdout.write(oracleSql());
  else if (process.argv.length === 4) {
    const receipt = verifyOracle(readFileSync(resolve(process.argv[2]), 'utf8'));
    writeFileSync(resolve(process.argv[3]), JSON.stringify(receipt, null, 2) + '\n');
    console.log(JSON.stringify({ status: receipt.status, assertions: receipt.checks.length, rows: receipt.rows }));
  } else throw Error('Usage: audit-error-proportion-oracle.mjs --sql | CAPTURE.tsv RECEIPT.json');
}
