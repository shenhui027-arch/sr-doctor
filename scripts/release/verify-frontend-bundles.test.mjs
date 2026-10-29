import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import test from "node:test";

import { LIMITS } from "./verify-frontend-bundles.mjs";

const script = resolve(import.meta.dirname, "verify-frontend-bundles.mjs");

function writeFixture(root) {
  const dist = join(root, "dist");
  const assets = join(dist, "assets");
  mkdirSync(assets, { recursive: true });
  writeFileSync(join(assets, "react-vendor-abc123.js"), "export const reactVendor = true;\n");
  writeFileSync(join(assets, "graph-vendor-def456.js"), "export const graphVendor = true;\n");
  writeFileSync(join(assets, "elk-worker.min-ghi789.js"), "export const elkWorker = true;\n");
  writeFileSync(join(assets, "graph-layout.worker-jkl012.js"), "import './elk-worker.min-ghi789.js';\n");
  writeFileSync(join(assets, "index-mno345.js"), "import './react-vendor-abc123.js'; import './graph-vendor-def456.js';\n");
  writeFileSync(join(dist, "index.html"), '<!doctype html><script type="module" src="/assets/index-mno345.js"></script>\n');
  return { dist, assets };
}

function run(dist, receipt, policy) {
  const args = [script, "--dist", dist];
  if (receipt) args.push("--receipt", receipt);
  if (policy) args.push("--policy", policy);
  return spawnSync(process.execPath, args, { encoding: "utf8", windowsHide: true });
}

function output(result) {
  assert.doesNotMatch(result.stderr, /^(?:Error|node:)/);
  return JSON.parse(result.stdout);
}

test("accepts a valid bundle layout and emits a compact receipt", () => {
  const temporary = mkdtempSync(join(tmpdir(), "frontend-bundles-"));
  try {
    const fixture = writeFixture(temporary);
    const receipt = join(temporary, "receipts", "bundle.json");
    const result = run(fixture.dist, receipt);
    assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
    const parsed = output(result);
    assert.equal(parsed.status, "PASS");
    assert.equal(parsed.entry.path, "assets/index-mno345.js");
    assert.equal(parsed.graph_path_bytes, 68);
    assert.equal(existsSync(receipt), true);
    assert.deepEqual(JSON.parse(readFileSync(receipt, "utf8")), parsed);
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
});

test("rejects an entry over the frozen byte budget", () => {
  const temporary = mkdtempSync(join(tmpdir(), "frontend-bundles-"));
  try {
    const fixture = writeFixture(temporary);
    writeFileSync(join(fixture.assets, "index-mno345.js"), Buffer.alloc(LIMITS.entry + 1, 0x20));
    const result = run(fixture.dist);
    assert.notEqual(result.status, 0);
    assert.equal(output(result).code, "entry_too_large");
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
});

test("counts all application chunks even when a small entry defers or omits their imports", () => {
  const temporary = mkdtempSync(join(tmpdir(), "frontend-bundles-"));
  try {
    const fixture = writeFixture(temporary);
    const entryBytes = Buffer.byteLength(readFileSync(join(fixture.assets, "index-mno345.js")));
    writeFileSync(join(fixture.assets, "AuditWorkloadWorkspace-pqr678.js"), Buffer.alloc(LIMITS.applicationJavaScript - entryBytes, 0x20));
    const accepted = run(fixture.dist);
    assert.equal(accepted.status, 0);
    assert.equal(output(accepted).application_javascript_bytes, LIMITS.applicationJavaScript);
    writeFileSync(join(fixture.assets, "hidden-extra-stu901.js"), " ");
    const rejected = run(fixture.dist);
    assert.equal(rejected.status, 1);
    assert.equal(output(rejected).code, "application_javascript_too_large");
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
});

test("rejects an entry that omits a required vendor reference", () => {
  const temporary = mkdtempSync(join(tmpdir(), "frontend-bundles-"));
  try {
    const fixture = writeFixture(temporary);
    writeFileSync(join(fixture.assets, "index-mno345.js"), "import './react-vendor-abc123.js';\n");
    const result = run(fixture.dist);
    assert.notEqual(result.status, 0);
    assert.equal(output(result).code, "entry_missing_graph_vendor_reference");
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
});

test("private v0.2 records real oversized assets as WARN without changing limits", () => {
  const temporary = mkdtempSync(join(tmpdir(), "frontend-bundles-"));
  try {
    const { dist, assets } = writeFixture(temporary);
    for (const name of ["index-mno345.js", "react-vendor-abc123.js", "graph-vendor-def456.js", "graph-layout.worker-jkl012.js", "elk-worker.min-ghi789.js"])
      writeFileSync(join(assets, name), readFileSync(join(assets, name), "utf8") + " ".repeat(1_800_000));
    const result = run(dist, null, "private-v0.2-size-observation");
    assert.equal(result.status, 0);
    const receipt = output(result);
    assert.equal(receipt.status, "WARN");
    assert.equal(receipt.policy, "private-v0.2-size-observation");
    assert.deepEqual(receipt.limits, LIMITS);
    assert.ok(receipt.entry.size_bytes > 1_800_000);
    assert.match(receipt.policy_decision, /user decision/);
    for (const code of ["entry_too_large", "react_vendor_too_large", "graph_vendor_too_large", "graph_worker_too_large", "elk_worker_too_large", "graph_path_too_large", "javascript_asset_too_large", "application_javascript_too_large"])
      assert.ok(receipt.warnings.some(warning => warning.code === code && warning.limit_bytes > 0));
    assert.equal(output(run(dist)).code, "entry_too_large");
  } finally { rmSync(temporary, { recursive: true, force: true }); }
});

test("observation policy still blocks missing assets, vendor and worker references", () => {
  const temporary = mkdtempSync(join(tmpdir(), "frontend-bundles-"));
  try {
    for (const [file, source, code] of [
      ["index-mno345.js", " ".repeat(LIMITS.entry + 1), "entry_missing_react_vendor_reference"],
      ["graph-layout.worker-jkl012.js", " ", "graph_worker_missing_elk_reference"],
      ["react-vendor-abc123.js", null, "react-vendor_count"],
    ]) {
      const { dist, assets } = writeFixture(temporary);
      if (source === null) rmSync(join(assets, file)); else writeFileSync(join(assets, file), source);
      const result = run(dist, null, "private-v0.2-size-observation");
      assert.equal(result.status, 1);
      assert.equal(output(result).code, code);
    }
    assert.equal(output(run(join(temporary, "dist"), null, "unknown")).code, "invalid_policy");
  } finally { rmSync(temporary, { recursive: true, force: true }); }
});

// The policy keeps its v0.2 name: it records the 2026-09-08 size-observation decision, not the product version.
test("current build and formal Candidate share the explicit size-observation policy", () => {
  const root = resolve(import.meta.dirname, "../..");
  const pkg = JSON.parse(readFileSync(join(root, "frontend/package.json"), "utf8"));
  assert.match(pkg.version, /^1\.0\./);
  assert.match(pkg.scripts.build, /npm run verify:bundles/);
  assert.match(pkg.scripts["verify:bundles"], /--policy private-v0\.2-size-observation/);
  const candidate = readFileSync(join(root, "scripts/release/candidate-lib.mjs"), "utf8");
  assert.match(candidate, /command\("npm", \["--prefix", "frontend", "run", "build"\], sourceRoot, null\)/);
});
