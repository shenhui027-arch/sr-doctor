import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const [inputPath, captureText, outputPath, manifestPath] = process.argv.slice(2);
const captureIndex = Number(captureText);
if (!inputPath || !Number.isInteger(captureIndex) || captureIndex < 1 || !outputPath || !manifestPath) {
  throw new Error("usage: node sanitize-explain-costs.mjs INPUT CAPTURE_INDEX OUTPUT MANIFEST");
}

const source = readFileSync(inputPath, "utf8");
const captureBlocks = source.split(/<!-- SR_PROFILE_CAPTURE\b/);
if (captureIndex >= captureBlocks.length) throw new Error("capture index is out of range");
const block = captureBlocks[captureIndex];
const explainHeading = block.indexOf("### EXPLAIN COSTS");
if (explainHeading < 0) throw new Error("capture has no EXPLAIN COSTS section");
const fenceStart = block.indexOf("```", explainHeading);
const jsonStart = block.indexOf("\n", fenceStart) + 1;
const fenceEnd = block.indexOf("```", jsonStart);
if (fenceStart < 0 || jsonStart <= 0 || fenceEnd < 0) throw new Error("EXPLAIN COSTS fence is incomplete");
const rows = JSON.parse(block.slice(jsonStart, fenceEnd));
if (!Array.isArray(rows) || rows.some((row) => typeof row?.["Explain String"] !== "string")) {
  throw new Error("EXPLAIN COSTS is not an Explain String row array");
}

const nodeLabels = new Set([
  "AGGREGATE",
  "ANALYTIC",
  "ASSERT NUMBER OF ROWS",
  "Decode",
  "EXCHANGE",
  "HASH JOIN",
  "MERGING-EXCHANGE",
  "OlapScanNode",
  "PARTITION-TOP-N",
  "Project",
  "SCAN MYSQL",
  "SELECT",
  "SORT",
  "TableValueFunction",
  "TOP-N",
  "UNION",
]);
const aggregateLabels = new Set([
  "AGGREGATE (update serialize)",
  "AGGREGATE (update finalize)",
  "AGGREGATE (merge finalize)",
]);
const distributions = new Set(["BROADCAST", "GATHER", "SHUFFLE"]);
const joinStrategies = new Set([
  "INNER JOIN (BROADCAST)",
  "INNER JOIN (PARTITIONED)",
  "RIGHT OUTER JOIN (BUCKET_SHUFFLE(S))",
  "RIGHT OUTER JOIN (PARTITIONED)",
]);

const emitted = [];
const nodeIdentities = new Set();
let fragment = null;
let nodeOccurrences = 0;
let acceptedNodeLabels = 0;
let unavailableNodeLabels = 0;
let factCandidates = 0;
let acceptedFactCandidates = 0;
let unavailableFactCandidates = 0;

for (const row of rows) {
  const line = row["Explain String"];
  const fragmentMatch = line.match(/^PLAN FRAGMENT ([0-9]+)\(F([0-9]+)\)$/);
  if (fragmentMatch) {
    fragment = `F${fragmentMatch[2]}`;
    emitted.push(line);
    continue;
  }
  if (/^\s*(?:RESULT SINK|STREAM DATA SINK|MultiCastDataSinks:?)$/.test(line)
    || /^\s*(?:OutPut Exchange Id|EXCHANGE ID): [0-9]+$/.test(line)) {
    emitted.push(line);
    continue;
  }
  if (/^[ |\-]*$/.test(line)) {
    emitted.push(line);
    continue;
  }

  const nodeMatch = line.match(/^([ |\-]*?)([0-9]+):(.+)$/);
  if (nodeMatch) {
    nodeOccurrences += 1;
    const label = nodeMatch[3];
    const accepted = nodeLabels.has(label) || aggregateLabels.has(label);
    if (accepted) {
      acceptedNodeLabels += 1;
      nodeIdentities.add(`${fragment ?? "no-fragment"}:${nodeMatch[2]}`);
      emitted.push(line);
    } else {
      unavailableNodeLabels += 1;
    }
    continue;
  }

  const factMatch = line.match(/^([ |\-]*)(cardinality|distribution type|join op):\s*(.*)$/);
  if (!factMatch) continue;
  factCandidates += 1;
  const [, , key, value] = factMatch;
  const accepted = key === "cardinality"
    ? /^[0-9]{1,39}$/.test(value)
    : key === "distribution type"
      ? distributions.has(value)
      : joinStrategies.has(value);
  if (accepted) {
    acceptedFactCandidates += 1;
    emitted.push(line);
  } else {
    unavailableFactCandidates += 1;
  }
}

const sanitized = `${emitted.join("\n")}\n`;
const forbidden = /(?:\bTABLE\s*:|predicates?\s*:|output exprs?\s*:|\bSQL\b|[A-Za-z]:\\|\/home\/|\bhost\s*:|\bpassword\s*:|\btoken\s*:|\b[0-9a-f]{8}-[0-9a-f-]{27,}\b)/i;
if (forbidden.test(sanitized)) throw new Error("sanitized output contains a forbidden token");
writeFileSync(outputPath, sanitized, "utf8");

const sha256 = (value) => createHash("sha256").update(value).digest("hex");
const tool = readFileSync(fileURLToPath(import.meta.url));
const manifest = {
  schema_version: "sr3514.explain_costs.sanitized_stats.v1",
  source: {
    label: "local-production-capture",
    sha256: sha256(Buffer.from(source, "utf8")),
    bytes: Buffer.byteLength(source, "utf8"),
    capture_count: captureBlocks.length - 1,
    selected_capture_index: captureIndex,
  },
  tool: {
    id: "sanitize-explain-costs.mjs",
    sha256: sha256(tool),
  },
  input: { explain_row_occurrences: rows.length },
  output: {
    sha256: sha256(Buffer.from(sanitized, "utf8")),
    emitted_row_occurrences: emitted.length,
    node_occurrences: nodeOccurrences,
    unique_node_identities: nodeIdentities.size,
    accepted_node_labels: acceptedNodeLabels,
    unavailable_node_labels: unavailableNodeLabels,
    fact_candidates: factCandidates,
    accepted_fact_candidates: acceptedFactCandidates,
    unavailable_fact_candidates: unavailableFactCandidates,
  },
  privacy: {
    original_text_committed: false,
    allowlisted_fields: [
      "fragment identity",
      "plan node identity",
      "registered node kind",
      "exchange identity",
      "cardinality integer",
      "distribution enum",
      "join strategy enum",
    ],
  },
};
writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
