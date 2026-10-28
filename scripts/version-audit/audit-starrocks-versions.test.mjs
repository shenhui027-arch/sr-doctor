import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { compareVersions, extractBlock, extractThriftBlocks, fingerprint, parseArgs, releaseTags, renderMarkdown, segments } from "./audit-starrocks-versions.mjs";

test("release tags keep only x.y.z of the audited lines, in semantic order", () => {
  const tags = ["3.1.10", "3.1.2", "3.1.0-rc01", "2.4.5", "4.1.7", "v3.5.0", "3.5.14", "3.10.0"];
  assert.deepEqual(releaseTags(tags, ["3.1", "3.5", "4.1"]), ["3.1.2", "3.1.10", "3.5.14", "4.1.7"]);
  assert.ok(compareVersions("3.1.10", "3.1.9") > 0);
});

test("a Java method is extracted by its header with balanced braces, ignoring braces in strings", () => {
  const source = [
    "class A {",
    "    // printCounter(long is mentioned here",
    "    private static String printCounter(long value, TUnit type) {",
    "        if (value > 0) { return \"}\"; }",
    "        return \"x\";",
    "    }",
    "    void other() {}",
    "}",
  ].join("\n");
  const block = extractBlock(source, "printCounter(long");
  assert.match(block, /^ {4}private static String printCounter/);
  assert.match(block, /return "x";\n {4}\}$/);
  assert.equal(extractBlock(source, "missing("), null);
});

test("a C++ definition is found, not its declaration or a call", () => {
  const source = [
    "void RuntimeProfile::merge(RuntimeProfile* other);",
    "auto x = RuntimeProfile::merge(a);",
    "void RuntimeProfile::merge(RuntimeProfile* other) {",
    "    for (auto& c : counters) { c.merge(); }",
    "}",
  ].join("\n");
  assert.equal(extractBlock(source, "RuntimeProfile::merge(", { cpp: true }), source.split("\n").slice(2).join("\n"));
});

test("thrift blocks are concatenated and an absent head is recorded", () => {
  const source = "enum TCounterMergeType {\n  MERGE_ALL = 0\n}\nstruct TCounterStrategy {\n 1: required i32 a\n}\n";
  assert.equal(extractThriftBlocks(source, ["enum TCounterMergeType", "enum TCounterMinMaxType"]), "enum TCounterMergeType {\n  MERGE_ALL = 0\n}\nenum TCounterMinMaxType: absent");
});

test("fingerprints ignore whitespace and comments but not code", () => {
  assert.equal(fingerprint("a(1);  // note\n b();"), fingerprint("a(1);\n/* x */ b();"));
  assert.notEqual(fingerprint("a(1);"), fingerprint("a(2);"));
  assert.equal(fingerprint(null), null);
});

test("consecutive equal values collapse into segments", () => {
  const values = { "3.1.0": "a", "3.1.1": "b", "3.1.2": "b", "3.1.4": "c" };
  assert.deepEqual(segments(Object.keys(values), (tag) => values[tag]), [
    { from: "3.1.0", to: "3.1.0", value: "a" },
    { from: "3.1.1", to: "3.1.2", value: "b" },
    { from: "3.1.4", to: "3.1.4", value: "c" },
  ]);
});

test("arguments are required and unknown flags are refused", () => {
  assert.throws(() => parseArgs([]), /usage/);
  assert.throws(() => parseArgs(["--repo"]), /needs a value/);
  assert.throws(() => parseArgs(["--repo", "r", "--out", "o", "--bogus"]), /unknown option/);
  assert.equal(parseArgs(["--repo", "r", "--out", "o", "--no-commits"]).commits, false);
});

test("the watchlist covers every supported line and names the baseline", () => {
  const watchlist = JSON.parse(readFileSync(new URL("./watchlist.json", import.meta.url), "utf8"));
  assert.equal(watchlist.baseline, "3.5.14");
  assert.deepEqual(watchlist.lines, ["3.1", "3.2", "3.3", "3.4", "3.5", "4.0", "4.1"]);
  for (const entry of watchlist.functions) assert.ok(entry.paths.length && (entry.match || entry.cpp || entry.block), entry.id);
});

test("the markdown marks baseline-identical segments and lists intervals", () => {
  const markdown = renderMarkdown({
    baseline: "3.5.14", tags: ["3.4.10", "3.5.14"], functions: [{ id: "fe.x", area: "a", baseline: "h1", segments: [{ from: "3.4.10", to: "3.4.10", value: "h0" }, { from: "3.5.14", to: "3.5.14", value: "h1" }] }],
    metrics: [], unlisted_metrics: [], capabilities: [], audit_fields: [], intervals: [{ from: "3.4.10", to: "3.5.14", total_commits: 10, watch_commits: 2, commits: [] }],
  });
  assert.match(markdown, /3\.4\.10 `h0`<br>3\.5\.14 ✅/);
  assert.match(markdown, /\| 3\.4\.10 → 3\.5\.14 \| 10 \| 2 \|/);
});
