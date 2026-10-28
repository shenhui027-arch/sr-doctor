import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { compareVersions } from "./audit-starrocks-versions.mjs";

// The version deltas the product reads must agree with the source audit they
// were derived from: a hand edit that drifts from the evidence fails here.
const deltas = JSON.parse(readFileSync("backend/crates/version-packs/starrocks_3_5_14/version-deltas.json", "utf8"));
const audit = JSON.parse(readFileSync("fixtures/version-audit/version-audit.json", "utf8"));
const pack = JSON.parse(readFileSync("backend/crates/version-packs/starrocks_3_5_14/version-pack.json", "utf8"));
const tags = audit.tags;
const delta = (id) => {
  const found = deltas.deltas.find((entry) => entry.id === id);
  assert.ok(found, `delta ${id} is missing`);
  return found;
};
const fn = (id) => audit.functions.find((entry) => entry.id === id);
const capability = (id) => audit.capabilities.find((entry) => entry.id === id);
// The product supports 3.1 and later; the audit (task 26) also covers 2.5 and 3.0.
const FIRST_SUPPORTED = "3.1.0";
const supported = (segments) => segments
  .filter((segment) => compareVersions(segment.to, FIRST_SUPPORTED) >= 0)
  .map((segment) => (compareVersions(segment.from, FIRST_SUPPORTED) >= 0 ? segment : { ...segment, from: FIRST_SUPPORTED }));
const ranges = (segments, predicate) => supported(segments).filter(predicate).map((segment) => [segment.from, segment.to]);
/** Adjacent segments that both match merge into one range. */
const mergedRanges = (segments, predicate) => {
  const out = [];
  for (const segment of supported(segments)) {
    const last = out.at(-1);
    const matches = predicate(segment);
    if (matches && last && last.open) last.range[1] = segment.to;
    else if (matches) out.push({ range: [segment.from, segment.to], open: true });
    else if (last) last.open = false;
  }
  return out.map((entry) => entry.range);
};
const next = (tag) => tags[tags.indexOf(tag) + 1];

test("the baseline and every line match the audited tags", () => {
  assert.equal(deltas.baseline_release, audit.baseline);
  assert.equal(deltas.baseline_commit, pack.upstream_commit);
  for (const line of deltas.lines) {
    const inLine = tags.filter((tag) => tag.startsWith(`${line.line}.`));
    assert.equal(line.audited_from, inLine[0], line.line);
    assert.equal(line.audited_to, inLine.at(-1), line.line);
    assert.ok(inLine.includes(line.smoke_release), `${line.line} smoke release is not audited`);
    assert.match(line.smoke_commit, /^[0-9a-f]{40}$/);
  }
});

test("only lines from 3.1 on are supported", () => {
  assert.deepEqual(deltas.lines.map((line) => line.line), ["3.1", "3.2", "3.3", "3.4", "3.5", "4.0", "4.1"]);
  for (const [from] of [...deltas.deltas.flatMap((entry) => entry.ranges), ...deltas.absent_metrics.flatMap((entry) => entry.ranges)]) {
    assert.ok(compareVersions(from, FIRST_SUPPORTED) >= 0, from);
  }
});

test("every range is ordered and bounded by audited tags, and every delta cites source", () => {
  const all = [...deltas.deltas.flatMap((entry) => entry.ranges), ...deltas.absent_metrics.flatMap((entry) => entry.ranges)];
  for (const [from, to] of all) {
    assert.ok(tags.includes(from) && tags.includes(to), `${from}..${to}`);
    assert.ok(compareVersions(from, to) <= 0, `${from}..${to}`);
  }
  for (const entry of deltas.deltas) {
    assert.ok(entry.evidence.length > 0 && entry.evidence.every((item) => item.path && item.symbol), entry.id);
    assert.ok(["adopt", "own"].includes(entry.decision), entry.id);
  }
});

test("absent metrics are exactly the audit's absent segments", () => {
  const registered = new Set(pack.metric_registry.metrics.map((entry) => entry.metric_id));
  const fromAudit = audit.metrics
    .map((entry) => ({ metric: entry.metric, ranges: ranges(entry.segments, (segment) => segment.value === "absent") }))
    .filter((entry) => entry.ranges.length);
  assert.deepEqual(deltas.absent_metrics, fromAudit);
  for (const entry of deltas.absent_metrics) assert.ok(registered.has(entry.metric), entry.metric);
});

test("capability deltas are exactly the audit's 'no' segments", () => {
  assert.deepEqual(delta("plan_advisor_unavailable").ranges, ranges(capability("plan_advisor").segments, (segment) => segment.value === "no"));
  // Every supported release has JSON Profiles, SHOW PROFILELIST and get_query_profile().
  for (const capabilityId of ["json_profile", "show_profilelist", "get_query_profile"]) {
    assert.deepEqual(ranges(capability(capabilityId).segments, (segment) => segment.value === "no"), [], capabilityId);
  }
});

test("text structure deltas are exactly the audit's legacy segments", () => {
  // The pre-3.1 execution section, query state and fragment layout never reach a supported release.
  for (const capabilityId of ["execution_section_named_execution", "query_state_profile_string", "fragment_instance_ids"]) {
    assert.deepEqual(ranges(capability(capabilityId).segments, (segment) => segment.value === "no"), [], capabilityId);
  }
  assert.deepEqual(delta("text_structure_negative_id_operator_header").ranges, ranges(capability("bare_operator_profile_name").segments, (segment) => segment.value === "yes"));
});

test("the legacy text format ends where the byte unit function reaches the baseline", () => {
  const byteUnit = fn("fe.byte_unit");
  assert.deepEqual(delta("text_counter_format_legacy").ranges, ranges(byteUnit.segments, (segment) => segment.value !== byteUnit.baseline));
});

test("time semantics and counter strategy deltas follow the audited functions", () => {
  // Every supported release ships the official ExplainAnalyzer and a counter strategy.
  assert.deepEqual(ranges(fn("fe.compute_time_usage").segments, (segment) => segment.value === "absent"), []);
  assert.deepEqual(ranges(fn("be.counter_strategy").segments, (segment) => segment.value === "absent"), []);
  assert.deepEqual(delta("official_output_rows_recipe_differs").ranges, ranges(fn("fe.search_output_rows").segments, (segment) => segment.value === "absent"));
  const strategy = fn("be.counter_strategy");
  assert.deepEqual(delta("counter_min_max_type_absent").ranges, ranges(strategy.segments, (segment) => segment.value !== "absent" && segment.value !== strategy.baseline));
});

test("each merged-max defect range ends right before the merge function returns to the baseline", () => {
  const merge = fn("fe.merge_isomorphic");
  const at = (tag) => merge.segments.find((segment) => compareVersions(segment.from, tag) <= 0 && compareVersions(tag, segment.to) <= 0).value;
  for (const [from, to] of delta("merged_max_from_min_defect").ranges) {
    assert.notEqual(at(from), merge.baseline, from);
    assert.notEqual(at(to), merge.baseline, to);
    assert.equal(at(next(to)), merge.baseline, `${next(to)} should carry the fix`);
  }
});

test("the per-BE CPU and spill fix covers exactly the releases whose query summary carries it", () => {
  const summary = fn("fe.build_query_profile");
  const fixed = summary.segments.at(-1).value;
  assert.notEqual(fixed, summary.baseline);
  assert.deepEqual(delta("query_summary_per_be_cpu_spill").ranges, mergedRanges(summary.segments, (segment) => segment.value === fixed));
});
