import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { SANITIZER_VERSION, assertNoSensitiveData, sanitizeRuntimeProfile } from "./sanitize-runtime-profile.mjs";

test("allowlist sanitizer reconstructs only RuntimeProfile structure and metrics", () => {
  const result = sanitizeRuntimeProfile(`Query:\n  Summary:\n\n    - Query ID: 6bf1bc11-1111-2222-3333-1234567890ab\n    - StarRocks Version: 3.5.14-deadbee\n    - Sql Statement: SELECT * FROM private_table\n  Execution:\n    - Topology: {"rootId":1,"nodes":[{"id":1,"name":"PROJECT","properties":{"secret":"no"},"children":[0]},{"id":0,"name":"OLAP_SCAN","properties":{},"children":[]}]}\n    Fragment 0:\n      - BackendAddresses: 10.0.0.1:9060\n      - InstanceIds: 6bf1bc11-1111-2222-3333-1234567890ab\n      Instance source-value:\n      Pipeline (id=0):\n        PipelineDriver (id=0):\n          OLAP_SCAN (plan_node_id=0):\n            CommonMetrics:\n              - OperatorTotalTime: 500ms\n              - UnknownMetric: internal-value\n`);
  assert.equal(SANITIZER_VERSION, "p04-runtime-profile-allowlist-v3");
  assert.match(result.output, /StarRocks Version: 3\.5\.14-23a56ec/);
  assert.match(result.output, /Topology: \{"rootId":1,"nodes":\[/);
  assert.match(result.output, /OperatorTotalTime: 500ms/);
  assert.match(result.output, /BackendAddresses: REDACTED_ENDPOINT/);
  assert.match(result.output, /InstanceIds: REDACTED_INSTANCE/);
  assert.match(result.output, /Instance REDACTED_0:/);
  assert.doesNotMatch(result.output, /Query ID|SELECT|private_table|secret|UnknownMetric/i);
  assert.equal(result.structure_counts.fragments, 1);
  assert.equal(result.structure_counts.pipelines, 1);
  assert.equal(result.structure_counts.drivers, 1);
  assert.equal(result.structure_counts.instances, 1);
  assert.equal(result.structure_counts.operator_lines, 1);
  assert.equal(result.structure_counts.metric_lines, 1);
  assert.equal(result.dropped_reason_counts.query_or_session_id, 1);
  assert.equal(result.dropped_reason_counts.sql_or_business_content, 1);
  assert.equal(result.dropped_reason_counts.blank_line, 1);
  assert.equal(result.retained_line_count + result.dropped_line_count, result.source_line_count);
});

test("SELECT operator identity remains publishable while SQL text stays denied", () => {
  const result = sanitizeRuntimeProfile(`Query:\n  Summary:\n    - StarRocks Version: 3.5.14-deadbee\n  Execution:\n    - Topology: {"rootId":1,"nodes":[{"id":1,"name":"SELECT","properties":{},"children":[]}]}\n    Fragment 0:\n      SELECT (plan_node_id=1):\n        CommonMetrics:\n          - OperatorTotalTime: 1ms\n`);

  assert.match(result.output, /"name":"SELECT"/);
  assert.match(result.output, /SELECT \(plan_node_id=1\):/);
  assert.doesNotThrow(() => assertNoSensitiveData(result.output));
  assert.throws(() => assertNoSensitiveData("Query:\n  - SELECT secret_column FROM private_table\n"), /sensitive-content denylist/);
});

test("denylist fails closed without returning sensitive source", () => {
  assert.throws(() => assertNoSensitiveData("Query:\n  - Sql Statement: SELECT x\n"), /sensitive-content denylist/);
});

test("unknown all-caps identities are not published as topology nodes or operators", () => {
  const result = sanitizeRuntimeProfile(`Query:\n  Summary:\n    - StarRocks Version: 3.5.14-deadbee\n  Execution:\n    - Topology: {"rootId":1,"nodes":[{"id":1,"name":"CUSTOMER_FACT","properties":{},"children":[]}]}\n    Fragment 0:\n      CUSTOMER_FACT (plan_node_id=1):\n        CommonMetrics:\n          - OperatorTotalTime: 1ms\n`);

  assert.doesNotMatch(result.output, /CUSTOMER_FACT/);
  assert.doesNotMatch(result.output, /Topology:/);
  assert.doesNotMatch(result.output, /OperatorTotalTime/);
  assert.equal(result.structure_counts.topology_lines, undefined);
  assert.equal(result.structure_counts.operator_lines, undefined);
  assert.equal(result.structure_counts.metric_lines, undefined);
  assert.equal(result.dropped_reason_counts.unknown_operator_subtree, 1);
});

test("CLI failures do not echo a sensitive source path", () => {
  const script = fileURLToPath(new URL("./sanitize-runtime-profile.mjs", import.meta.url));
  const sensitivePath = "E:\\private\\customer-profile.profile";
  const result = spawnSync(process.execPath, [
    script,
    "--input", sensitivePath,
    "--output", "fixtures/runtime-profiles/starrocks-3.5.14/p04/unused.profile.txt",
    "--manifest", "fixtures/manifests/unused.json",
  ], { encoding: "utf8" });

  assert.notEqual(result.status, 0);
  assert.equal(result.stderr, "fixture_sanitization_failed\n");
  assert.doesNotMatch(result.stderr, /private|customer-profile|E:\\/i);
});
