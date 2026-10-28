import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { oracleSql, verifyOracle } from '../e2e/audit-error-proportion-oracle.mjs';

// Real fixed-3.5.14 result of the read-only synthetic SELECT, not audit data.
const capture = readFileSync(new URL('../../fixtures/synthetic/error-proportion-sql-3514.tsv', import.meta.url), 'utf8');

test('fixed 3.5.14 DECIMAL capture agrees with exact BigInt ordering inside the bound', () => {
  const receipt = verifyOracle(capture);
  assert.equal(receipt.status, 'PASS');
  assert.equal(receipt.checks.length, 54);
  assert.equal(receipt.rows, 12);
});

test('oracle rejects an error-count ranking substituted for ratio ranking', () => {
  const rows = capture.trim().split(/\r?\n/);
  const a = rows.findIndex(r => r.includes('\ta_error_count_high\t'));
  const b = rows.findIndex(r => r.includes('\tb_ratio_high\t'));
  [rows[a], rows[b]] = [rows[b], rows[a]];
  assert.throws(() => verifyOracle(rows.join('\n')), /bounded_order/);
});

test('oracle rejects wrong population proof and corrupt/missing decimal results', () => {
  const rows = capture.trim().split(/\r?\n/).map(row => row.split('\t'));
  rows[0][4] = '0';
  assert.throws(() => verifyOracle(rows.map(row => row.join('\t')).join('\n')), /population_guard/);
  assert.throws(() => verifyOracle(capture.replace('1.0000000000000000000000000', '0.0000000000000000000000000')), /decimal_half_up/);
  assert.throws(() => verifyOracle(capture.trim().split(/\r?\n/).slice(1).join('\n')), /all_synthetic_rows/);
});

test('numeric experiment is one read-only SELECT over constants, not a table generator', () => {
  const sql = oracleSql();
  assert.match(sql, /MAX\(canonical_execution_count\) OVER/);
  assert.match(sql, /SET_VAR\(query_timeout = 10\)/);
  assert.match(sql, /DECIMAL\(38,25\)/);
  assert.match(sql, /DECIMAL\(38,0\)/);
  assert.doesNotMatch(sql, /\b(?:CREATE|DROP|INSERT|DELETE|UPDATE|TRUNCATE)\b/i);
  assert.match(sql, /FROM family_rankable ORDER BY population ASC, sort_key DESC, family_id ASC/);
});
