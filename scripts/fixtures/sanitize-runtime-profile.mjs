#!/usr/bin/env node
/**
 * Fail-closed derivation for a locally supplied RuntimeProfile.  It deliberately
 * reconstructs a small grammar rather than redacting arbitrary source lines.
 * The input is never copied to a repository artifact or an error message.
 */
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

export const SANITIZER_VERSION = "p04-runtime-profile-allowlist-v3";
const METRICS = new Set([
  "ActiveTime", "DriverTotalTime", "PendingTime", "ScheduleTime", "ScheduleCount",
  "OperatorTotalTime", "ScanTime", "NetworkTime", "PrepareTime", "PushRowNum",
  "PullRowNum", "OutputRows", "OutputChunkBytes", "SpillBytes", "QueryExecutionWallTime",
  "QueryCumulativeOperatorTime", "QueryCumulativeScanTime", "QueryCumulativeNetworkTime",
]);
// Keep this deliberately narrower than the parser grammar.  Sanitization is a
// publication boundary: an unknown all-caps token may be a table or business
// identifier, so it must be dropped instead of being treated as an operator.
const OPERATOR_TYPES = new Set([
  "AGGREGATE", "ANALYTIC_EVAL", "ASSERT_NUM_ROWS", "ASSERT_NUM_ROWS_SINK",
  "CHUNK_ACCUMULATE", "EMPTY_SET", "EXCHANGE", "EXCHANGE_SINK", "EXCHANGE_SOURCE",
  "HASH_JOIN", "LOCAL_EXCHANGE_SINK", "LOCAL_EXCHANGE_SOURCE", "MERGE_JOIN",
  "NESTLOOP_JOIN", "NOOP_SINK", "OLAP_SCAN", "OLAP_SCAN_PREPARE", "PROJECT",
  "REPEAT", "RESULT_SINK", "SELECT", "SORT", "TABLE_FUNCTION", "TOP_N", "UNION",
]);
const IDENTITY = /\b(query\s*id|session|user|host|address|backend|instance|default\s+db|database)\b/i;
const OUTPUT_IDENTITY = /\b(query\s*id|session|user|host|default\s+db|database)\b/i;
const SQL = /\b(select|insert|update|delete|alter|create|drop|sql\s+statement)\b/i;
const OUTPUT_SQL = /\b(insert|update|delete|alter|create|drop|sql\s+statement)\b/i;
const SECRET = /\b(password|credential|secret|token)\b/i;
const PATH = /(?:[A-Za-z]:[\\/]|\/home\/|\\\\)/;
const UUID = /\b[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}\b/i;
const DENYLIST = [OUTPUT_IDENTITY, OUTPUT_SQL, SECRET, PATH, UUID, /\b(?:\d{1,3}\.){3}\d{1,3}\b/];

function hash(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function increment(map, key) {
  map[key] = (map[key] ?? 0) + 1;
}

function droppedReason(line) {
  if (SECRET.test(line)) return "credential_or_secret";
  if (PATH.test(line)) return "path";
  if (UUID.test(line)) return "query_or_session_id";
  if (SQL.test(line)) return "sql_or_business_content";
  if (IDENTITY.test(line)) return "identity_or_endpoint";
  return "unclassified";
}

function sanitizedTopology(text) {
  try {
    const parsed = JSON.parse(text);
    if (!Number.isSafeInteger(parsed.rootId) || !Array.isArray(parsed.nodes)) return null;
    const nodes = parsed.nodes.map((node) => {
      if (!Number.isSafeInteger(node?.id) || !OPERATOR_TYPES.has(node?.name) || !Array.isArray(node?.children) || !node.children.every(Number.isSafeInteger)) return null;
      return { id: node.id, name: node.name, properties: { sinkIds: [] }, children: node.children };
    });
    return nodes.every(Boolean) ? JSON.stringify({ rootId: parsed.rootId, nodes }) : null;
  } catch {
    return null;
  }
}

function safeLine(line, counts) {
  const indent = line.match(/^\s*/)?.[0] ?? "";
  const trimmed = line.trim();
  if (["Query:", "Summary:", "Planner:", "Execution:", "CommonMetrics:", "UniqueMetrics:"].includes(trimmed)) return indent + trimmed;
  if (/^Fragment \d+:$/.test(trimmed)) {
    increment(counts, "fragments");
    return indent + trimmed;
  }
  if (/^Pipeline \(id=\d+\):$/.test(trimmed)) {
    increment(counts, "pipelines");
    return indent + trimmed;
  }
  if (/^PipelineDriver \(id=\d+\):$/.test(trimmed)) {
    increment(counts, "drivers");
    return indent + trimmed;
  }
  if (/^Instance .+:$/.test(trimmed)) {
    const ordinal = counts.instances ?? 0;
    increment(counts, "instances");
    return `${indent}Instance REDACTED_${ordinal}:`;
  }
  if (/^- Query Type: Query$/.test(trimmed) || /^- Query State: (Finished|Running|Failed)$/.test(trimmed)) return indent + trimmed;
  if (/^- StarRocks Version: 3\.5\.14(?:-[0-9a-f]{7,40})?$/i.test(trimmed)) {
    increment(counts, "version_lines");
    return `${indent}- StarRocks Version: 3.5.14-23a56ec`;
  }
  const topology = trimmed.match(/^- Topology:\s*(\{.*\})$/);
  if (topology) {
    const normalized = sanitizedTopology(topology[1]);
    if (normalized) {
      increment(counts, "topology_lines");
      return `${indent}- Topology: ${normalized}`;
    }
    return null;
  }
  const operator = trimmed.match(/^([A-Z][A-Z0-9_]*)(?: \(plan_node_id=(-?\d+)\))?:$/);
  if (operator && operator[2] !== undefined && OPERATOR_TYPES.has(operator[1])) {
    increment(counts, "operator_lines");
    return `${indent}${operator[1]} (plan_node_id=${operator[2]}):`;
  }
  if (/^- BackendAddresses:\s*.+$/.test(trimmed)) {
    increment(counts, "backend_address_markers");
    return `${indent}- BackendAddresses: REDACTED_ENDPOINT`;
  }
  if (/^- InstanceIds:\s*.+$/.test(trimmed)) {
    increment(counts, "instance_id_markers");
    return `${indent}- InstanceIds: REDACTED_INSTANCE`;
  }
  if (/^- (?:IsSubordinate|IsFinalSink)$/.test(trimmed)) return indent + trimmed;
  const metric = trimmed.match(/^- ([A-Za-z][A-Za-z0-9]*):\s*(-?\d+(?:\.\d+)?(?:ns|us|ms|s|min|h|B|KB|MB|GB)?)(?:\s*\([^)]*\))?$/);
  if (metric && METRICS.has(metric[1])) {
    increment(counts, "metric_lines");
    return `${indent}- ${metric[1]}: ${metric[2]}`;
  }
  return null;
}

export function sanitizeRuntimeProfile(input) {
  const text = Buffer.isBuffer(input) ? input.toString("utf8") : input;
  const normalized = text.replace(/\r\n/g, "\n");
  const sourceLines = normalized.endsWith("\n") ? normalized.slice(0, -1).split("\n") : normalized.split("\n");
  const allowed = [];
  const structure_counts = {};
  const dropped_reason_counts = {};
  let dropped_line_count = 0;
  let blockedOperatorIndent = null;
  for (const line of sourceLines) {
    const indentLength = line.match(/^\s*/)?.[0].length ?? 0;
    if (blockedOperatorIndent !== null && line.trim()) {
      if (indentLength > blockedOperatorIndent) {
        dropped_line_count += 1;
        increment(dropped_reason_counts, droppedReason(line));
        continue;
      }
      blockedOperatorIndent = null;
    }
    const operatorCandidate = line.trim().match(/^([A-Z][A-Z0-9_]*) \(plan_node_id=-?\d+\):$/);
    if (operatorCandidate && !OPERATOR_TYPES.has(operatorCandidate[1])) {
      blockedOperatorIndent = indentLength;
      dropped_line_count += 1;
      increment(dropped_reason_counts, "unknown_operator_subtree");
      continue;
    }
    const safe = safeLine(line, structure_counts);
    if (safe !== null) allowed.push(safe);
    else {
      dropped_line_count += 1;
      increment(dropped_reason_counts, line.trim() ? droppedReason(line) : "blank_line");
    }
  }
  while (allowed.length && !allowed.at(-1)) allowed.pop();
  const output = `${allowed.join("\n")}\n`;
  assertNoSensitiveData(output);
  return {
    output,
    source_line_count: sourceLines.length,
    retained_line_count: allowed.filter(Boolean).length,
    dropped_line_count,
    dropped_reason_counts,
    structure_counts,
  };
}

export function assertNoSensitiveData(text) {
  for (const expression of DENYLIST) {
    if (expression.test(text)) throw new Error("derived fixture failed sensitive-content denylist");
  }
  if (text.split(/\r?\n/).some((line) => {
    if (!/\bselect\b/i.test(line)) return false;
    const trimmed = line.trim();
    if (/^SELECT \(plan_node_id=-?\d+\):$/.test(trimmed)) return false;
    const topology = trimmed.match(/^- Topology:\s*(\{.*\})$/);
    return !topology || sanitizedTopology(topology[1]) !== topology[1];
  })) {
    throw new Error("derived fixture failed sensitive-content denylist");
  }
  if (text.split(/\r?\n/).some((line) => {
    const trimmed = line.trim();
    return (trimmed.startsWith("Instance ") && !/^Instance REDACTED_\d+:$/.test(trimmed))
      || (trimmed.startsWith("- BackendAddresses:") && trimmed !== "- BackendAddresses: REDACTED_ENDPOINT")
      || (trimmed.startsWith("- InstanceIds:") && trimmed !== "- InstanceIds: REDACTED_INSTANCE");
  })) {
    throw new Error("derived fixture failed structural placeholder check");
  }
  if (!text.includes("StarRocks Version: 3.5.14")) throw new Error("derived fixture lacks an allowlisted version line");
}

function usage() {
  return "usage: node scripts/fixtures/sanitize-runtime-profile.mjs --input <absolute-source> --output <repo-fixture> --manifest <repo-manifest> | --check --output <repo-fixture> --manifest <repo-manifest>";
}

function options(args) {
  const parsed = { check: false };
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--check") parsed.check = true;
    else if (["--input", "--output", "--manifest"].includes(argument)) {
      if (parsed[argument.slice(2)]) throw new Error(`duplicate ${argument}`);
      parsed[argument.slice(2)] = args[++index];
    } else throw new Error(usage());
  }
  if (!parsed.output || !parsed.manifest || (!parsed.check && !parsed.input)) throw new Error(usage());
  return parsed;
}

async function main() {
  const parsed = options(process.argv.slice(2));
  const outputPath = path.resolve(parsed.output);
  const manifestPath = path.resolve(parsed.manifest);
  if (parsed.check) {
    const output = await readFile(outputPath, "utf8");
    assertNoSensitiveData(output);
    const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
    const receipt = manifest.sanitization;
    const derivedLines = output.endsWith("\n") ? output.slice(0, -1).split("\n").length : output.split("\n").length;
    if (!receipt || receipt.sanitizer_version !== SANITIZER_VERSION || receipt.derived_sha256 !== hash(output) || receipt.derived_bytes !== Buffer.byteLength(output) || receipt.derived_lines !== derivedLines) {
      throw new Error("complex profile sanitization receipt is stale");
    }
    return;
  }
  const source = await readFile(path.resolve(parsed.input));
  const result = sanitizeRuntimeProfile(source);
  const manifest = existsSync(manifestPath) ? JSON.parse(await readFile(manifestPath, "utf8")) : { schema_version: "1.0", fixture_family: "starrocks-3.5.14-complex-profile-regression", tiers: [] };
  manifest.sanitization = {
    sanitizer_version: SANITIZER_VERSION,
    source_sha256: hash(source),
    source_bytes: source.length,
    source_lines: result.source_line_count,
    retained_line_count: result.retained_line_count,
    dropped_line_count: result.dropped_line_count,
    dropped_reason_counts: result.dropped_reason_counts,
    structure_counts: result.structure_counts,
    derived_path: path.relative(process.cwd(), outputPath).replaceAll("\\", "/"),
    derived_sha256: hash(result.output),
    derived_bytes: Buffer.byteLength(result.output),
    derived_lines: result.output.slice(0, -1).split("\n").length,
  };
  await mkdir(path.dirname(outputPath), { recursive: true });
  await writeFile(outputPath, result.output, "utf8");
  await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(() => {
    // This CLI may read a sensitive source outside the repository.  Node I/O
    // failures include the absolute path, so the publication boundary emits
    // only a stable stage code.
    process.stderr.write("fixture_sanitization_failed\n");
    process.exitCode = 1;
  });
}
