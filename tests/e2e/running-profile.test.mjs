import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, mkdirSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { randomUUID } from "node:crypto";
import { runInNewContext, Script } from "node:vm";
import { assertRunningCounts, exactRunningCandidateIndex, resolveRunningProfileOptions, runningObserverSource, runningOfficialResponseMatches, awaitRunningIdentity, validateRunningIdentity } from "./running-profile.mjs";

const target = "12345678-1234-1234-1234-123456789abc";
function handshakeDirectory(t) {
  const directory = join(tmpdir(), `task54-running-${randomUUID().replaceAll("-", "")}`);
  mkdirSync(directory);
  t.after(() => { assert.equal(dirname(directory), tmpdir().replace(/[\\/]$/, "")); rmSync(directory, { recursive: true }); });
  return directory;
}
test("Handshake options reject dual configuration and non-unique/relative directories", t => {
  const directory = handshakeDirectory(t);
  assert.equal(resolveRunningProfileOptions(true, "connected-sql-docker", null, false, directory).handshakeDir, directory);
  for (const [qid, dir] of [[target, directory], [null, "task54-running-" + "a".repeat(32)], [null, tmpdir()]])
    assert.throws(() => resolveRunningProfileOptions(true, "connected-sql-docker", qid, false, dir));
});
test("Ready is atomic, identity consumed once and waiting preserves zero product requests", async t => {
  const directory = handshakeDirectory(t);
  let checks = 0;
  const result = await awaitRunningIdentity(directory, async () => { checks++; }, { sleep: async () => {
    const ready = JSON.parse(readFileSync(join(directory, "ready.json"), "utf8"));
    assert.equal(ready.schema, "task54-running-handshake/v1");
    assert.equal(existsSync(join(directory, "ready.json.tmp")), false);
    assert.deepEqual(Object.keys(ready).sort(), ["ready_at_utc", "schema"]);
    writeFileSync(join(directory, "identity.json"), JSON.stringify({ schema: ready.schema, query_id: target, observed_at_utc: new Date().toISOString() }));
  } });
  assert.equal(result, target);
  assert.ok(checks >= 3);
  await assert.rejects(awaitRunningIdentity(directory, async () => {}), /single-use/);
});
test("Handshake timeout is bounded and malformed or extra sensitive fields are rejected", async t => {
  let clock = Date.now();
  await assert.rejects(awaitRunningIdentity(handshakeDirectory(t), async () => {}, { now: () => clock, sleep: async () => { clock += 15000; } }), /45 seconds/);
  const ready = new Date().toISOString();
  const valid = { schema: "task54-running-handshake/v1", query_id: target, observed_at_utc: ready };
  for (const value of [{ ...valid, sql: "not accepted" }, { ...valid, query_id: "bad" }, { ...valid, observed_at_utc: "invalid" }, { ...valid, observed_at_utc: "2000-01-01T00:00:00Z" }])
    assert.throws(() => validateRunningIdentity(value, ready));
  await assert.rejects(awaitRunningIdentity(handshakeDirectory(t), async () => { throw new Error("nonzero routes"); }), /nonzero routes/);
});
test("Observer identity binds once without resetting request telemetry", () => {
  const window = { fetch: async () => ({ status: 200 }) };
  runInNewContext(runningObserverSource(null), { window, URL, location: { href: "http://127.0.0.1:8000" } });
  const records = window.__doctorE2eRunning.records;
  assert.throws(() => window.__doctorE2eRunning.bind("bad"));
  window.__doctorE2eRunning.bind(target);
  assert.equal(window.__doctorE2eRunning.records, records);
  assert.throws(() => window.__doctorE2eRunning.bind(target));
  const second = { fetch: window.fetch };
  runInNewContext(runningObserverSource(null), { window: second, URL, location: { href: "http://127.0.0.1:8000" } });
  second.__doctorE2eRunning.records.push({ kind: "list" });
  assert.throws(() => second.__doctorE2eRunning.bind(target));
  assert.equal(second.__doctorE2eRunning.records.length, 1);
});
test("Running opt-in is exclusive, real and requires an externally prepared exact identity", () => {
  assert.equal(resolveRunningProfileOptions(false, "offline-local"), null);
  assert.equal(resolveRunningProfileOptions(true, "connected-sql-docker", target).transport, "real_loopback_api");
  for (const args of [[true, "offline-local", target], [true, "connected-sql-docker", ""], [true, "connected-sql-docker", target, true]])
    assert.throws(() => resolveRunningProfileOptions(...args));
});
test("External UUID matching accepts casing but rejects substring, duplicate and absent identities", () => {
  assert.equal(exactRunningCandidateIndex(["other", target], target), 1);
  assert.equal(exactRunningCandidateIndex([target.toUpperCase()], target), 0);
  for (const ids of [[], [target + "suffix"], [target, target], [target, target.toUpperCase()]])
    assert.throws(() => exactRunningCandidateIndex(ids, target));
});
test("Only explicitly expected counts pass; fallback and duplicate calls fail", () => {
  assertRunningCounts({ list: 1, snapshot: 0, official: 0, forbidden: 0 }, 1, 0, 0);
  assert.throws(() => assertRunningCounts({ list: 2, snapshot: 0, official: 0, forbidden: 0 }, 1, 0, 0));
  assert.throws(() => assertRunningCounts({ list: 1, snapshot: 0, official: 0, forbidden: 1 }, 1, 0, 0));
});
test("Observer follows real fetch, checks discovery identity and never exports identity values", async () => {
  const connection = "a".repeat(48);
  const candidate = { query_id: target, connection_id: connection, discovery_generation: 7, discovery_source: "profile_list", status: "rUNNING" };
  let forwarded = 0;
  const original = async (url) => { forwarded++; return { status: 200, ok: true, clone: () => ({ json: async () => url.endsWith('official-analysis') ? officialResponse(candidate) : ({ discovery_generation: 7, candidates: [candidate] }) }) }; };
  const window = { fetch: original };
  const source = runningObserverSource(target);
  assert.doesNotThrow(() => new Script(source));
  runInNewContext(source, { window, URL, location: { href: "http://127.0.0.1:8000" } });
  const post = (route, body) => window.fetch(`/api/v1/${route}`, { body: JSON.stringify(body) });
  await post("profiles/running-candidates", { connection_id: connection });
  await post("profiles/official-analysis", { connection_id: connection, query_id: target, identity: { kind: "discovery", discovery_generation: 7 }, node_ids: [] });
  await post("profiles/running-snapshot", { connection_id: connection, query_id: target, discovery_generation: 6 });
  await post("runtime-profile", {});
  assert.equal(forwarded, 4);
  const rows = window.__doctorE2eRunning.records;
  assert.equal(rows[0].exact_candidate, true);
  assert.equal(rows[1].identity_matches, true);
  assert.equal(rows[1].response_identity_matches, true);
  assert.equal(rows[2].identity_matches, false);
  assert.equal(rows[3].kind, "forbidden");
  assert.equal(JSON.stringify(rows).includes(connection), false);
  assert.equal(JSON.stringify(rows).includes(target), false);
  window.__doctorE2eRunning.restore();
  assert.equal(window.fetch, original);
});

function officialResponse(owner) {
  return { kind: "analyze_profile_text", query: { ...owner }, identity: { kind: "discovery", discovery_generation: owner.discovery_generation }, node_ids: [] };
}
test("Official response checks exact owner and actual nested identity fields", () => {
  const owner = { connection_id: "connection", query_id: target, discovery_generation: 7 };
  assert.equal(runningOfficialResponseMatches(officialResponse(owner), owner), true);
  for (const mutate of [
    r => { r.query.query_id = target.toUpperCase(); },
    r => { r.query.connection_id = "other"; },
    r => { r.query.discovery_generation = 8; },
    r => { r.identity.discovery_generation = 8; },
    r => { r.identity = { kind: "verified_standard_attempt", execution_attempt_id: target }; },
    r => { r.identity.extra = true; },
    r => { r.node_ids = [0]; },
    r => { r.query_id = target; delete r.query; },
  ]) {
    const response = officialResponse(owner);
    mutate(response);
    assert.equal(runningOfficialResponseMatches(response, owner), false);
  }
});
test("Observer freezes request owner across refresh and preserves exact selected UUID casing", async () => {
  const candidate = { query_id: target.toUpperCase(), connection_id: "connection", discovery_generation: 7, discovery_source: "profile_list", status: "Running" };
  const prior = { ...candidate };
  let finishOfficial;
  const window = { fetch: async url => {
    if (url.endsWith('official-analysis')) await new Promise(resolve => { finishOfficial = resolve; });
    return { status: 200, ok: true, clone: () => ({ json: async () => url.endsWith('official-analysis') ? officialResponse(prior) : ({ discovery_generation: candidate.discovery_generation, candidates: [candidate] }) }) };
  } };
  runInNewContext(runningObserverSource(target), { window, URL, location: { href: "http://127.0.0.1:8000" } });
  const post = (route, body) => window.fetch(`/api/v1/${route}`, { body: JSON.stringify(body) });
  await post("profiles/running-candidates", { connection_id: "connection" });
  const pending = post("profiles/official-analysis", { connection_id: "connection", query_id: prior.query_id, identity: { kind: "discovery", discovery_generation: 7 }, node_ids: [] });
  candidate.discovery_generation = 8;
  await post("profiles/running-candidates", { connection_id: "connection" });
  finishOfficial();
  await pending;
  assert.equal(window.__doctorE2eRunning.records[1].response_identity_matches, true);
  await post("profiles/running-snapshot", { connection_id: "connection", query_id: target, discovery_generation: 8 });
  assert.equal(window.__doctorE2eRunning.records[3].identity_matches, false);
});
test("Formal harness keeps the legacy flow and uses shared screenshot redaction", () => {
  const main = readFileSync(new URL("./browser-e2e.mjs", import.meta.url), "utf8");
  const module = readFileSync(new URL("./running-profile.mjs", import.meta.url), "utf8");
  assert.match(main, /else if \(runningProfileOptions\)/);
  assert.match(main, /await clickButton\("Queries"\);\n      await topologyAndRuntimeCoverage\(\);/);
  assert.match(module, /await capture\("running-r4-retained-stale", \{ redactSql: true \}\)/);
  assert.match(main, /\.running-profile-connection code/);
  assert.match(main, /\.running-profile-entry pre\.official-profile-display-text/);
  assert.match(module, /document\.querySelectorAll\('\.running-profile-retained button'\)/);
  assert.match(module, /Open diagnosis/);
  assert.match(module, /\.workspace-doctor \.official-profile-open/);
  assert.match(module, /retained_doctor_readonly_no_io: true/);
  assert.ok(main.indexOf("await evaluate(runningObserverSource(runningProfileOptions.queryId))") < main.indexOf("await keyboardLocaleAndWorkspaceCoverage()"));
  assert.doesNotMatch(module, /isolatedFixtureSql|controlled_mock|setInterval|nativeExplainAnalyze\(/);
});

test("Shared redaction accepts input-free Running and preserves identity guards", async () => {
  const main = readFileSync(new URL("./browser-e2e.mjs", import.meta.url), "utf8");
  const source = main.slice(main.indexOf("async function redactVisibleIdentifiersForEvidence("), main.indexOf("async function capture("));
  const run = (redaction) => runInNewContext(`(${source})({ redactSql: true })`, {
    evaluate: async () => ({ aliases: 1, connection_aliases: 1, sql_inputs_redacted: 0,
      full_uuid_visible: false, full_connection_id_visible: false, ...redaction }),
    assert: (condition, message) => assert.ok(condition, message),
    receipt: { assertions: [] },
  });
  await assert.doesNotReject(run({}));
  await assert.rejects(run({ full_uuid_visible: true }), /Query UUID/);
  await assert.rejects(run({ full_connection_id_visible: true }), /Connection ID/);
});
