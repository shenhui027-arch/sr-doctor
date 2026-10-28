#!/usr/bin/env node
/**
 * G2: desensitizes StarRocks 3.5.14 Profile text exported from production
 * (`get_query_profile` output, one Profile per file).
 *
 * Unlike scripts/fixtures/sanitize-runtime-profile.mjs, which allowlists a
 * minimal public fixture, this keeps every metric and the full structure so
 * the sample can calibrate rules and clustering. It only rewrites values that
 * can carry business data:
 *   - "Sql Statement" (and its continuation lines): literals masked;
 *   - expression text (predicates, grouping/sort keys, aggregate functions,
 *     column lists): literals masked, identifiers pseudonymized on request;
 *   - user / host / address values: keyed pseudonyms;
 *   - table / rollup / partition / default db: kept, or pseudonymized with
 *     --pseudonymize-identifiers;
 *   - JSON text values (for example NonDefaultSessionVariables): keys,
 *     numbers and booleans kept, non-empty strings replaced, so the analyzer
 *     can still read settings such as pipeline_profile_level;
 *   - any other text value: quoted strings masked;
 *   - everywhere: IPv4 and e-mail addresses pseudonymized.
 * Numeric metric values and the Topology JSON are never touched.
 */
import { readdirSync, readFileSync, writeFileSync, mkdirSync, statSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { loadKey, parseArgs, pseudonymizeIdentities, sha256, writeReceipt } from "./calibration-io.mjs";
import { MASK_VERSION, maskSql, pseudonym } from "./sql-mask.mjs";

export const PROFILE_DESENSITIZER_VERSION = "g2-profile-v2";

const KEY_VALUE = /^(\s*)- ([^:]+?):(?:\s(.*))?$/;
const SQL_KEY = /^sql statement$/i;
const IDENTITY_KEY = /(^user$|address|host|^client|^fe ?ip|^node)/i;
const IDENTIFIER_KEY = /^(default db|table|tables|rollup|partition|partitions|partitionnames|index|mv|materialized ?view)/i;
// Values holding SQL expressions: column references, functions and literals.
export const EXPRESSION_KEY = /(predicat|conjunct|expr|filter ?desc|runtime ?filter.*desc|keys$|functions?$|columns?$|order ?by|partition ?by)/i;
const STRUCTURE = /^\s*[A-Za-z][\w ()=#.,-]*:\s*$/;
const METRIC_VALUE = /^-?\d+(?:\.\d+)?\s*(?:[A-Za-z%/]+)?(?:\s*\(.*\))?$/;
const QUOTED = /'(?:[^'\\]|\\.|'')*'|"(?:[^"\\]|\\.|"")*"/g;
const NETWORK_ADDRESS = /TNetworkAddress\(hostname:([^,)]+)/g;

function parseJson(text) {
  if (!/^[[{]/.test(text)) return undefined;
  try { return JSON.parse(text); } catch { return undefined; }
}

function maskJsonStrings(value) {
  if (typeof value === "string") return value === "" ? "" : "S";
  if (Array.isArray(value)) return value.map(maskJsonStrings);
  if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([name, item]) => [name, maskJsonStrings(item)]));
  return value;
}

export function desensitizeProfile(text, { key, pseudonymizeIdentifiers = false }) {
  const name = (prefix, value) => pseudonym(key, prefix, value);
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const out = [];
  const counts = { sql_blocks: 0, sql_continuation_lines: 0, expression_values: 0, json_values: 0, identity_values: 0, identifier_values: 0, text_values: 0, metric_lines: 0, unterminated_sql: 0 };
  const identity = (value) => value.split(",").map((part) => {
    const trimmed = part.trim();
    if (!trimmed) return part;
    const [host, ...rest] = trimmed.split(":");
    return name("h", host) + (rest.length ? `:${rest.join(":")}` : "");
  }).join(", ");
  const identifiers = (value) => (pseudonymizeIdentifiers ? value.split(",").map((part) => (part.trim() ? name("i", part.trim()) : part)).join(",") : value);
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    const match = line.match(KEY_VALUE);
    if (match && SQL_KEY.test(match[2].trim())) {
      // Gather continuation lines: everything up to the next "- key:" or section header.
      const block = [match[3] ?? ""];
      let j = i + 1;
      while (j < lines.length && !KEY_VALUE.test(lines[j]) && !STRUCTURE.test(lines[j])) { block.push(lines[j]); j += 1; }
      const masked = maskSql(block.join("\n"), { key, pseudonymizeIdentifiers });
      if (masked.stats.unterminated) counts.unterminated_sql += 1;
      counts.sql_blocks += 1;
      counts.sql_continuation_lines += block.length - 1;
      const maskedLines = pseudonymizeIdentities(masked.text, name).split("\n");
      out.push(`${match[1]}- ${match[2]}: ${maskedLines[0]}`.trimEnd(), ...maskedLines.slice(1));
      i = j - 1;
      continue;
    }
    if (match && match[3] !== undefined) {
      const [, indent, rawKey, value] = match;
      const keyName = rawKey.trim();
      let next;
      let json;
      if (keyName === "Topology") next = value;
      else if (METRIC_VALUE.test(value.trim())) { counts.metric_lines += 1; next = value; }
      else if (IDENTITY_KEY.test(keyName)) { counts.identity_values += 1; next = identity(value); }
      else if (IDENTIFIER_KEY.test(keyName)) { counts.identifier_values += 1; next = identifiers(value); }
      else if (EXPRESSION_KEY.test(keyName)) { counts.expression_values += 1; next = maskSql(value, { key, pseudonymizeIdentifiers }).text; }
      else if ((json = parseJson(value.trim())) !== undefined) { counts.json_values += 1; next = JSON.stringify(maskJsonStrings(json)); }
      else { counts.text_values += 1; next = value.replace(QUOTED, (quoted) => `${quoted[0]}S${quoted[0]}`); }
      out.push(pseudonymizeIdentities(`${indent}- ${rawKey}: ${next}`, name).replace(NETWORK_ADDRESS, (_, host) => `TNetworkAddress(hostname:${name("h", host.trim())}`));
      continue;
    }
    out.push(pseudonymizeIdentities(line, name).replace(NETWORK_ADDRESS, (_, host) => `TNetworkAddress(hostname:${name("h", host.trim())}`));
  }
  return { text: out.join("\n"), counts };
}

async function main() {
  const args = parseArgs(process.argv.slice(2), {
    "input-dir": "value", "output-dir": "value", "key-file": "value", "create-key": "flag",
    "pseudonymize-identifiers": "flag", receipt: "value",
  });
  if (!args["input-dir"] || !args["output-dir"]) throw new Error("usage: --input-dir raw/ --output-dir out/ --key-file key.bin [--create-key] [--pseudonymize-identifiers] [--receipt receipt.json]");
  const key = loadKey(args["key-file"], { create: Boolean(args["create-key"]) });
  const files = readdirSync(args["input-dir"]).filter((file) => statSync(path.join(args["input-dir"], file)).isFile()).sort();
  mkdirSync(args["output-dir"], { recursive: true });
  const results = [];
  for (const file of files) {
    const input = readFileSync(path.join(args["input-dir"], file));
    const result = desensitizeProfile(input.toString("utf8"), { key, pseudonymizeIdentifiers: Boolean(args["pseudonymize-identifiers"]) });
    writeFileSync(path.join(args["output-dir"], file), result.text);
    results.push({ file, input_sha256: sha256(input), output_sha256: sha256(Buffer.from(result.text)), counts: result.counts });
  }
  writeReceipt(args.receipt, {
    tool: PROFILE_DESENSITIZER_VERSION, mask: MASK_VERSION,
    pseudonymize_identifiers: Boolean(args["pseudonymize-identifiers"]), files: results,
  });
  process.stdout.write(`${JSON.stringify({ status: "done", files: results.length, unterminated_sql: results.reduce((sum, r) => sum + r.counts.unterminated_sql, 0) })}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => { process.stderr.write(`desensitize-profile: ${error.message}\n`); process.exitCode = 1; });
}
