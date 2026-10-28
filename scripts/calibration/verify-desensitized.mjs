#!/usr/bin/env node
/**
 * G2: fail-closed check of desensitized samples before they leave the
 * exporting machine. Exit 0 only when nothing is found.
 *
 * Audit TSV: only allowlisted columns; identity columns are pseudonyms; stmt
 * has no literal left; free text has no IPv4, e-mail, phone-like or
 * ID-card-like value; no term of the local denylist appears anywhere.
 * Profiles: the same text checks on "Sql Statement" blocks and text values,
 * IPv4 / e-mail / denylist over the whole file. Metric lines are excluded from
 * the phone/ID patterns, where long digit runs are ordinary byte counts.
 *
 * The receipt records rule names, files and line numbers, never content.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { IDENTITY_PATTERNS, TEXT_PATTERNS, parseArgs, parseBatchTsv, sha256, writeReceipt } from "./calibration-io.mjs";
import { columnPolicy } from "./desensitize-audit.mjs";
import { EXPRESSION_KEY } from "./desensitize-profile.mjs";
import { residualLiterals } from "./sql-mask.mjs";

export const VERIFIER_VERSION = "g2-verify-v1";
const PSEUDONYM_VALUE = /^p_[0-9a-f]{8}$/;
const DERIVED = new Set(["stmt_bytes", "stmt_unterminated"]);
const KEY_VALUE = /^(\s*)- ([^:]+?):(?:\s(.*))?$/;
const METRIC_VALUE = /^-?\d+(?:\.\d+)?\s*(?:[A-Za-z%/]+)?(?:\s*\(.*\))?$/;
const PSEUDO_TOKEN = /\b(?:ip|mail|h|i|p)_[0-9a-f]{8}\b/g;

function matches(pattern, text) {
  pattern.lastIndex = 0;
  return pattern.test(text.replace(PSEUDO_TOKEN, ""));
}

function textViolations(text, { freeText, denylist }) {
  const found = [];
  for (const [rule, pattern] of Object.entries(IDENTITY_PATTERNS)) if (matches(pattern, text)) found.push(rule);
  if (freeText) for (const [rule, pattern] of Object.entries(TEXT_PATTERNS)) if (matches(pattern, text)) found.push(rule);
  const lower = text.toLowerCase();
  if (denylist.some((term) => lower.includes(term))) found.push("denylist");
  return found;
}

export function verifyAudit(text, { denylist = [] } = {}) {
  const violations = [];
  const add = (rule, row, column) => violations.push({ rule, row, column });
  const { header, rows } = parseBatchTsv(text);
  header.forEach((name) => { if (!DERIVED.has(name) && columnPolicy(name) === "drop") add("unexpected_column", 1, name); });
  rows.forEach((row, index) => {
    const rowNumber = index + 2;
    header.forEach((name, column) => {
      const value = row[column];
      if (value === null || value === "") return;
      const policy = DERIVED.has(name) ? "derived" : columnPolicy(name);
      if (policy === "pseudonym" && !PSEUDONYM_VALUE.test(value)) add("identity_not_pseudonymized", rowNumber, name);
      if (policy === "sql") for (const rule of residualLiterals(value)) add(rule, rowNumber, name);
      for (const rule of textViolations(value, { freeText: policy === "sql" || policy === "identifier", denylist })) add(rule, rowNumber, name);
    });
  });
  return { rows: rows.length, violations };
}

export function verifyProfile(text, { denylist = [] } = {}) {
  const violations = [];
  const lines = text.split("\n");
  let inSql = false;
  lines.forEach((line, index) => {
    const lineNumber = index + 1;
    const match = line.match(KEY_VALUE);
    if (match) inSql = /^sql statement$/i.test(match[2].trim());
    const value = match ? match[3] ?? "" : line;
    const metric = Boolean(match) && METRIC_VALUE.test(value.trim());
    if (inSql) for (const rule of residualLiterals(value)) violations.push({ rule, line: lineNumber });
    else if (match && EXPRESSION_KEY.test(match[2].trim())) for (const rule of residualLiterals(value).filter((rule) => rule === "string_literal")) violations.push({ rule, line: lineNumber });
    const isTopology = Boolean(match) && match[2].trim() === "Topology";
    for (const rule of textViolations(line, { freeText: !metric && !isTopology, denylist })) violations.push({ rule, line: lineNumber });
  });
  return { violations };
}

function summarize(violations) {
  return violations.reduce((acc, violation) => { acc[violation.rule] = (acc[violation.rule] ?? 0) + 1; return acc; }, {});
}

async function main() {
  const args = parseArgs(process.argv.slice(2), { audit: "value", profiles: "value", denylist: "value", receipt: "value" });
  if (!args.audit && !args.profiles) throw new Error("usage: [--audit out.tsv] [--profiles out-dir/] [--denylist terms.txt] [--receipt receipt.json]");
  const denylist = args.denylist
    ? readFileSync(args.denylist, "utf8").split(/\r?\n/).map((term) => term.trim().toLowerCase()).filter((term) => term && !term.startsWith("#"))
    : [];
  const receipt = { tool: VERIFIER_VERSION, denylist_terms: denylist.length, denylist_sha256: args.denylist ? sha256(readFileSync(args.denylist)) : null, audit: null, profiles: [] };
  let total = 0;
  if (args.audit) {
    const input = readFileSync(args.audit);
    const result = verifyAudit(input.toString("utf8"), { denylist });
    total += result.violations.length;
    receipt.audit = { file: path.basename(args.audit), sha256: sha256(input), rows: result.rows, by_rule: summarize(result.violations), first: result.violations.slice(0, 50) };
  }
  if (args.profiles) {
    for (const file of readdirSync(args.profiles).filter((name) => statSync(path.join(args.profiles, name)).isFile()).sort()) {
      const input = readFileSync(path.join(args.profiles, file));
      const result = verifyProfile(input.toString("utf8"), { denylist });
      total += result.violations.length;
      receipt.profiles.push({ file, sha256: sha256(input), by_rule: summarize(result.violations), first: result.violations.slice(0, 20) });
    }
  }
  receipt.status = total === 0 ? "PASS" : "FAIL";
  receipt.violation_count = total;
  writeReceipt(args.receipt, receipt);
  process.stdout.write(`${JSON.stringify({ status: receipt.status, violations: total })}\n`);
  if (total > 0) process.exitCode = 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => { process.stderr.write(`verify-desensitized: ${error.message}\n`); process.exitCode = 2; });
}
