#!/usr/bin/env node
/**
 * G2: desensitizes a `mysql --batch` export of the AuditLoader table.
 *
 * Column policy (by header name, case-insensitive). Unknown columns are
 * dropped, fail-closed, and named in the receipt.
 *   keep        numeric facts, state, IDs, official digest
 *   pseudonym   user / client / FE identity, always (keyed, stable)
 *   identifier  db / catalog / MV names: kept, or pseudonymized with
 *               --pseudonymize-identifiers
 *   sql         stmt: literals masked (sql-mask.mjs)
 * Two derived columns are appended for the calibration: stmt_bytes (UTF-8
 * length before masking, to spot truncation) and stmt_unterminated.
 */
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { formatBatchTsv, loadKey, parseArgs, parseBatchTsv, pseudonymizeIdentities, sha256, writeReceipt } from "./calibration-io.mjs";
import { MASK_VERSION, maskSql, pseudonym } from "./sql-mask.mjs";

export const AUDIT_DESENSITIZER_VERSION = "g2-audit-v1";

const KEEP = new Set([
  "queryid", "timestamp", "querytype", "state", "errorcode", "querytime", "scanbytes", "scanrows",
  "returnrows", "cpucostns", "memcostbytes", "stmtid", "isquery", "digest", "plancpucosts",
  "planmemcosts", "pendingtimems", "resourcegroup", "warehouse", "querysource",
]);
const PSEUDONYM = new Set(["user", "authorizeduser", "clientip", "feip", "host"]);
const IDENTIFIER = new Set(["db", "catalog", "candidatemvs", "hitmvs"]);
const SQL = new Set(["stmt"]);

export function columnPolicy(name) {
  const lower = name.toLowerCase();
  if (KEEP.has(lower)) return "keep";
  if (PSEUDONYM.has(lower)) return "pseudonym";
  if (IDENTIFIER.has(lower)) return "identifier";
  if (SQL.has(lower)) return "sql";
  return "drop";
}

export function desensitizeAudit(text, { key, pseudonymizeIdentifiers = false }) {
  const { header, rows } = parseBatchTsv(text);
  const policies = header.map(columnPolicy);
  const kept = header.map((name, index) => ({ name, policy: policies[index], index })).filter((column) => column.policy !== "drop");
  const stmtIndex = policies.indexOf("sql");
  const outHeader = [...kept.map((column) => column.name), ...(stmtIndex >= 0 ? ["stmt_bytes", "stmt_unterminated"] : [])];
  const totals = { rows: rows.length, strings: 0, numbers: 0, comments: 0, identifiers: 0, unterminated_rows: 0, null_stmt_rows: 0 };
  const name = (prefix, value) => pseudonym(key, prefix, value);
  const outRows = rows.map((row) => {
    const out = kept.map(({ policy, index }) => {
      const value = row[index];
      if (value === null || value === "") return value;
      switch (policy) {
        case "keep": return value;
        case "pseudonym": return name("p", value);
        case "identifier":
          return pseudonymizeIdentifiers ? value.split(",").map((part) => (part.trim() ? name("i", part.trim()) : part)).join(",") : value;
        case "sql": {
          const masked = maskSql(value, { key, pseudonymizeIdentifiers });
          for (const field of ["strings", "numbers", "comments", "identifiers"]) totals[field] += masked.stats[field];
          if (masked.stats.unterminated) totals.unterminated_rows += 1;
          return pseudonymizeIdentities(masked.text, name);
        }
        default: return null;
      }
    });
    if (stmtIndex >= 0) {
      const stmt = row[stmtIndex];
      if (stmt === null) totals.null_stmt_rows += 1;
      out.push(stmt === null ? null : String(Buffer.byteLength(stmt, "utf8")));
      out.push(stmt === null ? null : String(maskSql(stmt).stats.unterminated));
    }
    return out;
  });
  return {
    text: formatBatchTsv(outHeader, outRows),
    columns: header.map((column, index) => ({ name: column, policy: policies[index] })),
    totals,
  };
}

async function main() {
  const args = parseArgs(process.argv.slice(2), {
    input: "value", output: "value", "key-file": "value", "create-key": "flag",
    "pseudonymize-identifiers": "flag", receipt: "value",
  });
  if (!args.input || !args.output) throw new Error("usage: --input audit.tsv --output out.tsv --key-file key.bin [--create-key] [--pseudonymize-identifiers] [--receipt receipt.json]");
  const key = loadKey(args["key-file"], { create: Boolean(args["create-key"]) });
  const input = readFileSync(args.input);
  const result = desensitizeAudit(input.toString("utf8"), { key, pseudonymizeIdentifiers: Boolean(args["pseudonymize-identifiers"]) });
  mkdirSync(path.dirname(path.resolve(args.output)), { recursive: true });
  writeFileSync(args.output, result.text);
  const receipt = {
    tool: AUDIT_DESENSITIZER_VERSION, mask: MASK_VERSION,
    pseudonymize_identifiers: Boolean(args["pseudonymize-identifiers"]),
    input_sha256: sha256(input), output_sha256: sha256(Buffer.from(result.text)),
    columns: result.columns, totals: result.totals,
  };
  writeReceipt(args.receipt, receipt);
  process.stdout.write(`${JSON.stringify({ status: "done", rows: result.totals.rows, dropped_columns: result.columns.filter((column) => column.policy === "drop").map((column) => column.name), unterminated_rows: result.totals.unterminated_rows })}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => { process.stderr.write(`desensitize-audit: ${error.message}\n`); process.exitCode = 1; });
}
