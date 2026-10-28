import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { desensitizeAudit } from "./desensitize-audit.mjs";
import { desensitizeProfile } from "./desensitize-profile.mjs";
import { DOCTOR_MARKER, maskSql, residualLiterals } from "./sql-mask.mjs";
import { verifyAudit, verifyProfile } from "./verify-desensitized.mjs";

const key = Buffer.alloc(32, 7);
const here = path.dirname(new URL(import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1"));

test("masks every literal kind and keeps the statement shape", () => {
  const sql = `${DOCTOR_MARKER} SELECT a, "b x", 'it''s', 'c\\'d' FROM t1 WHERE id IN (1, 2, 3) AND v > -1.5e3 AND h = 0x1F AND p < .5 -- tail\n# hash\n/* note 13812345678 */`;
  const { text, stats } = maskSql(sql);
  assert.equal(text, `${DOCTOR_MARKER} SELECT a, "S", 'S', 'S' FROM t1 WHERE id IN (0, 0, 0) AND v > -0 AND h = 0 AND p < 0 --\n--\n/* */`);
  assert.deepEqual({ ...stats, unterminated: stats.unterminated }, { strings: 3, numbers: 6, comments: 3, identifiers: 0, unterminated: false });
  assert.deepEqual(residualLiterals(text), []);
});

test("a truncated statement masks everything after the open quote and says so", () => {
  const { text, stats } = maskSql("SELECT * FROM t WHERE name = 'Zhang San 1381234");
  assert.equal(text, "SELECT * FROM t WHERE name = 'S'");
  assert.equal(stats.unterminated, true);
});

test("identifier pseudonyms are stable, skip keywords and functions, and keep backticks", () => {
  const masked = maskSql("SELECT COUNT(*) AS n, `Order Id` FROM sales.orders o JOIN sales.orders p ON o.id = p.id", { key, pseudonymizeIdentifiers: true }).text;
  assert.match(masked, /^SELECT COUNT\(\*\) AS i_[0-9a-f]{8}, `i_[0-9a-f]{8}` FROM i_[0-9a-f]{8}\.i_[0-9a-f]{8} i_[0-9a-f]{8} JOIN /);
  const orders = masked.match(/FROM (i_[0-9a-f]{8}\.i_[0-9a-f]{8})/)[1];
  assert.ok(masked.includes(`JOIN ${orders} `), "the same table gets the same pseudonym");
  assert.ok(!/sales|orders|Order/.test(masked));
});

const auditTsv = [
  "queryId\ttimestamp\tuser\tclientIp\tdb\tstate\tqueryTime\tscanBytes\tdigest\tisQuery\tstmt\tmysteryColumn",
  "q-1\t2026-10-22 07:00:00\talice\t10.1.2.3:51234\tsales\tEOF\t263\t13912345678\tabc123\t1\tSELECT * FROM t WHERE phone = '13812345678'\\nAND id = 42\tsecret-value",
  "q-2\t2026-10-22 07:00:01\tNULL\tNULL\tNULL\tERR\t5\t0\tNULL\t0\tNULL\tx",
].join("\n") + "\n";

test("audit export: identities pseudonymized, stmt masked, unknown columns dropped, metrics untouched", () => {
  const result = desensitizeAudit(auditTsv, { key });
  const [header, first, second] = result.text.trim().split("\n").map((line) => line.split("\t"));
  assert.deepEqual(header, ["queryId", "timestamp", "user", "clientIp", "db", "state", "queryTime", "scanBytes", "digest", "isQuery", "stmt", "stmt_bytes", "stmt_unterminated"]);
  assert.match(first[2], /^p_[0-9a-f]{8}$/);
  assert.match(first[3], /^p_[0-9a-f]{8}$/);
  assert.equal(first[4], "sales");
  assert.equal(first[7], "13912345678", "a byte count is a metric, not a phone number");
  assert.equal(first[10], "SELECT * FROM t WHERE phone = 'S'\\nAND id = 0");
  assert.equal(first[11], String(Buffer.byteLength("SELECT * FROM t WHERE phone = '13812345678'\nAND id = 42")));
  assert.equal(first[12], "false");
  assert.deepEqual(second.slice(2, 5), ["NULL", "NULL", "NULL"]);
  assert.ok(!result.text.includes("secret-value") && !result.text.includes("alice"));
  assert.deepEqual(result.columns.find((column) => column.name === "mysteryColumn"), { name: "mysteryColumn", policy: "drop" });
  assert.deepEqual(verifyAudit(result.text, { denylist: ["sales"] }).violations.map((v) => v.rule), ["denylist"]);
  assert.deepEqual(verifyAudit(result.text).violations, []);
});

test("audit verifier catches an export that was not desensitized", () => {
  const rules = verifyAudit(auditTsv).violations.map((violation) => violation.rule).sort();
  assert.ok(rules.includes("unexpected_column"));
  assert.ok(rules.includes("identity_not_pseudonymized"));
  assert.ok(rules.includes("string_literal"));
  assert.ok(rules.includes("numeric_literal"));
  assert.ok(rules.includes("phone_like"));
  assert.ok(rules.includes("ipv4"));
});

const profile = [
  "Query:",
  "  Summary:",
  "     - Query ID: 217ed49c-cde9-11f1-ba89-5e6e68288ee6",
  "     - User: alice",
  "     - Default Db: sales",
  "     - Sql Statement: SELECT * FROM orders",
  "WHERE customer = 'Acme Ltd' AND mobile = '13812345678'",
  "     - Variables: resource_group='vip'",
  "     - NonDefaultSessionVariables: {\"pipeline_profile_level\":{\"defaultValue\":1,\"actualValue\":2},\"resource_group\":{\"defaultValue\":\"\",\"actualValue\":\"vip_team\"}}",
  "  Execution:",
  "     - Topology: {\"rootId\":1,\"nodes\":[{\"id\":1,\"name\":\"OLAP_SCAN\",\"children\":[]}]}",
  "    Fragment 0:",
  "       - BackendAddresses: 172.17.0.2:9060, be-host-7:9060",
  "      OLAP_SCAN (plan_node_id=0):",
  "        UniqueMetrics:",
  "           - Table: orders",
  "           - Predicates: 3: customer = 'Acme Ltd', 4: total > 1000",
  "           - BytesRead: 13912345678",
  "           - OperatorTotalTime: 1.234ms",
  "      AGGREGATE (plan_node_id=1):",
  "           - GroupingKeys: [5: customer_name, 6: date_trunc('day', 7: created_at)]",
  "       - Instance 1 (host=TNetworkAddress(hostname:10.0.0.9, port:9060)):",
].join("\n");

test("Profile: metrics and Topology untouched; SQL, predicates and identities rewritten", () => {
  const { text, counts } = desensitizeProfile(profile, { key });
  const lines = text.split("\n");
  for (const keep of ["     - Query ID: 217ed49c-cde9-11f1-ba89-5e6e68288ee6", "     - Topology: {\"rootId\":1,\"nodes\":[{\"id\":1,\"name\":\"OLAP_SCAN\",\"children\":[]}]}", "           - BytesRead: 13912345678", "           - OperatorTotalTime: 1.234ms", "     - Default Db: sales", "           - Table: orders"]) {
    assert.ok(lines.includes(keep), keep);
  }
  assert.ok(lines.includes("     - Sql Statement: SELECT * FROM orders"));
  assert.ok(lines.includes("WHERE customer = 'S' AND mobile = 'S'"));
  assert.ok(lines.includes("     - Variables: resource_group='S'"));
  // Session-variable JSON stays valid JSON with its keys and numbers, so the
  // analyzer can still read pipeline_profile_level; string values are replaced.
  assert.ok(lines.includes("     - NonDefaultSessionVariables: {\"pipeline_profile_level\":{\"defaultValue\":1,\"actualValue\":2},\"resource_group\":{\"defaultValue\":\"\",\"actualValue\":\"S\"}}"));
  assert.ok(!text.includes("vip_team"));
  assert.ok(lines.includes("           - Predicates: 0: customer = 'S', 0: total > 0"));
  assert.ok(lines.includes("           - GroupingKeys: [0: customer_name, 0: date_trunc('S', 0: created_at)]"));
  // With identifier pseudonyms, expression keys rename columns like the SQL does; functions stay.
  const renamed = desensitizeProfile(profile, { key, pseudonymizeIdentifiers: true }).text;
  assert.match(renamed, /- GroupingKeys: \[0: i_[0-9a-f]{8}, 0: date_trunc\('S', 0: i_[0-9a-f]{8}\)\]/);
  assert.ok(!/customer_name|created_at/.test(renamed));
  assert.match(text, /- User: h_[0-9a-f]{8}/);
  assert.match(text, /- BackendAddresses: h_[0-9a-f]{8}:9060, h_[0-9a-f]{8}:9060/);
  assert.match(text, /hostname:h_[0-9a-f]{8}, port:9060/);
  assert.ok(!/Acme|alice|172\.17|10\.0\.0\.9|be-host-7|13812345678/.test(text));
  assert.equal(counts.sql_blocks, 1);
  assert.equal(counts.sql_continuation_lines, 1);
  assert.deepEqual(verifyProfile(text).violations, []);
  assert.ok(verifyProfile(text, { denylist: ["orders"] }).violations.some((violation) => violation.rule === "denylist"));
});

test("Profile verifier catches a raw Profile", () => {
  const rules = new Set(verifyProfile(profile).violations.map((violation) => violation.rule));
  for (const rule of ["string_literal", "ipv4", "phone_like"]) assert.ok(rules.has(rule), rule);
});

test("command line: creates the key on request and writes receipts without sample content", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "g2-"));
  try {
    writeFileSync(path.join(dir, "audit.tsv"), auditTsv);
    mkdirSync(path.join(dir, "raw"));
    writeFileSync(path.join(dir, "raw", "q1.profile"), profile);
    const keyFile = path.join(dir, "private", "g2.key");
    execFileSync(process.execPath, [path.join(here, "desensitize-audit.mjs"), "--input", path.join(dir, "audit.tsv"), "--output", path.join(dir, "out", "audit.tsv"), "--key-file", keyFile, "--create-key", "--receipt", path.join(dir, "out", "audit.receipt.json")]);
    assert.ok(existsSync(keyFile));
    execFileSync(process.execPath, [path.join(here, "desensitize-profile.mjs"), "--input-dir", path.join(dir, "raw"), "--output-dir", path.join(dir, "out", "profiles"), "--key-file", keyFile, "--receipt", path.join(dir, "out", "profiles.receipt.json")]);
    const verify = execFileSync(process.execPath, [path.join(here, "verify-desensitized.mjs"), "--audit", path.join(dir, "out", "audit.tsv"), "--profiles", path.join(dir, "out", "profiles"), "--receipt", path.join(dir, "out", "verify.receipt.json")]).toString();
    assert.match(verify, /"status":"PASS"/);
    for (const receipt of ["audit.receipt.json", "profiles.receipt.json", "verify.receipt.json"]) {
      const body = readFileSync(path.join(dir, "out", receipt), "utf8");
      assert.ok(!/alice|Acme|13812345678|secret-value|10\.1\.2\.3/.test(body), receipt);
    }
    // A raw file fails the verifier with exit 1.
    assert.throws(() => execFileSync(process.execPath, [path.join(here, "verify-desensitized.mjs"), "--audit", path.join(dir, "audit.tsv")], { stdio: "pipe" }), (error) => error.status === 1);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
