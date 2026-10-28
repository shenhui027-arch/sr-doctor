import test from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { createHash, webcrypto } from "node:crypto";
import { readFileSync, mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { auditReleaseObserverSource, containsAuditSql, statementFilterDomMatches, verifyAuditReleaseOracle, verifyAuditReleaseReceipt } from "./audit-release.mjs";

const root = new URL("./", import.meta.url);
const scope = { window: { start_seconds: 10, end_seconds: 3610 }, database: null, user: "workload_user", statement_filter: "exclude_doctor_and_non_query" };
const statementFilter = { state: "applied", hidden_non_query_row_count: "4", hidden_doctor_row_count: "2" };
function fixture() {
  const fields = { scope, sort: "query_time_desc", audit_timestamp_time_zone: null, retained_event_count: "0", error_count: "0", statement_filter: statementFilter, top: { requested_limit: 100, returned_count: 0, full_window_count: "0", truncated: false }, rows: [] };
  return { observation: { schema: "audit-release-observation/v1", transport: "real_loopback_api", overview: { body: { ...fields, sql_leak: false }, statement_filter_dom: { state: "applied", text: "Hidden: 4 non-query or unclassified audit rows (isQuery ≠ 1) and 2 rows sent by this tool." }, columns: ["Query ID", "SQL", "Start / reader zone", "Source state", "Query duration", "Scanned rows", "Returned rows", "Scanned bytes", "CPU time", "Audit memory", "Profile"], column_titles: ["", "", "", "", "queryTime", "scanRows", "returnRows", "scanBytes", "cpuCost", "memoryCost", ""], rows: [] } }, oracle: { schema: "audit-release-oracle/v1", source: "fixed_3.5.14_auditloader_independent_sql", status: "PASS", cohort_complete: true, ...fields, dom_rows: [] } };
}
test("same-window oracle compares exact counters and rejects scope/filter/rank/DOM drift", () => {
  const { observation, oracle } = fixture();
  assert.equal(verifyAuditReleaseOracle(observation, oracle).status, "PASS");
  for (const patch of [{ scope: { ...scope, database: "other" } }, { retained_event_count: "9007199254740993" }, { sort: "scan_bytes_desc" }, { dom_rows: [{}] }, { status: "INCONCLUSIVE" }, { cohort_complete: false }, { statement_filter: { ...statementFilter, hidden_doctor_row_count: "3" } }, { statement_filter: { state: "not_applied" } }, { scope: { ...scope, statement_filter: "all" } }]) {
    assert.throws(() => verifyAuditReleaseOracle(observation, { ...oracle, ...patch }));
  }
  assert.throws(() => verifyAuditReleaseOracle({ ...observation, transport: "controlled_mock" }, oracle));
  // Rendered disclosure must carry the independent hidden counts, not just a state.
  assert.throws(() => verifyAuditReleaseOracle({ ...observation, overview: { ...observation.overview, statement_filter_dom: { state: "applied", text: "Hidden rows" } } }, oracle));
  assert.throws(() => verifyAuditReleaseOracle({ ...observation, overview: { ...observation.overview, statement_filter_dom: null } }, oracle));
  const unscoped = structuredClone(observation);
  delete unscoped.overview.body.scope.statement_filter;
  assert.throws(() => verifyAuditReleaseOracle(unscoped, { ...oracle, scope: unscoped.overview.body.scope }));
});
test("statement-filter disclosure accepts only exact server counts or an explicit not-applied state", () => {
  assert.equal(statementFilterDomMatches({ state: "applied", text: "Hidden: 4 … and 2 …" }, statementFilter), true);
  assert.equal(statementFilterDomMatches({ state: "applied", text: "Hidden: 4 … and 0 …" }, statementFilter), false);
  assert.equal(statementFilterDomMatches({ state: "not_applied", text: "Showing all statements" }, { state: "not_applied" }), true);
  assert.equal(statementFilterDomMatches({ state: "not_applied", text: "" }, statementFilter), false);
  assert.equal(statementFilterDomMatches(null, { state: "not_applied" }), false);
  assert.equal(statementFilterDomMatches({ state: "applied", text: "Hidden: -1 and 2" }, { ...statementFilter, hidden_non_query_row_count: "-1" }), false);
});
test("batch leak scanner catches nested SQL/credentials without serializing them", () => {
  for (const key of ["sql", "stmt", "statement", "sql_text", "password", "credential"]) assert.equal(containsAuditSql({ rows: [{ [key]: "secret" }] }), true);
  assert.equal(containsAuditSql({ rows: [{ query_id: "id", metrics: [] }] }), false);
});
test("observer delegates the identical request exactly once and stores only projected evidence", async () => {
  const calls = [];
  const ids = ["query_time", "scan_rows", "return_rows", "scan_bytes", "cpu_cost", "memory_cost"];
  const body = { scope, coverage: { retained_event_count: "1", error_count: "0", statement_filter: statementFilter }, primary: { state: "occurrences", top: {}, rows: [{ query: { query_id: "ABC", connection_id: "secret", status: "finished" }, metrics: ids.map(metric_id => ({ metric_id, state: "exact", canonical_number: { value: "1" }, canonical_unit: "item" })) }] } };
  const response = new Response(JSON.stringify(body));
  const window = { fetch: async (...args) => { calls.push(args); return response; } };
  vm.runInNewContext(auditReleaseObserverSource(), { window, URL, Request, location: { href: "http://127.0.0.1/" }, crypto: webcrypto, TextEncoder, performance });
  const init = { body: JSON.stringify({ request: { primary: { sort: "query_time_desc" } } }) };
  assert.equal(await window.fetch("/api/v1/connections/secret/audit/overview", init), response);
  assert.equal(calls.length, 1); assert.equal(calls[0][1], init);
  const record = window.__auditRelease.records[0];
  assert.match(record.rows[0].query_id_sha256, /^[0-9a-f]{64}$/);
  assert.equal(JSON.stringify(record).includes("secret"), false);
  assert.equal(JSON.stringify(record).includes("ABC"), false);
  assert.equal(record.sort, "query_time_desc");
  assert.deepEqual(record.statement_filter, statementFilter);
});
test("real F05 explicit actions retain family facts and do not turn absent evidence into PASS", () => {
  const source = readFileSync(new URL("browser-e2e.mjs", root), "utf8");
  assert.match(source, /real_typed_unavailable_facts_retained/);
  assert.match(source, /real_success_unavailability_not_exercised/);
  assert.match(source, /Representative Profile failure triggered SQL or execution fallback/);
  assert.match(source, /familyFactsBeforeEvidence/);
});
test("A05 applies the real immutable user cohort before capture then restores legacy scope", () => {
  const source = readFileSync(new URL("browser-e2e.mjs", root), "utf8");
  const apply = source.indexOf('await applyAuditReleaseCohort("workload_user")');
  const capture = source.indexOf('await captureAuditReleaseOverview');
  const reset = source.indexOf('await applyAuditReleaseCohort("")');
  assert.ok(apply > 0 && apply < capture && capture < reset);
  assert.match(source, /A05 requires the real exact-user filter capability/);
  assert.match(source, /A05 explicit one-hour exact-user scope committed/);
  const { observation, oracle } = fixture();
  const unfiltered = structuredClone(observation);
  unfiltered.overview.body.scope.user = null;
  assert.throws(() => verifyAuditReleaseOracle(unfiltered, { ...oracle, scope: unfiltered.overview.body.scope }));
});
test("checker CLI writes real terminal evidence and does not disclose mismatched values", () => {
  const dir = mkdtempSync(join(tmpdir(), "audit-release-test-"));
  try {
    const { observation, oracle } = fixture();
    const receiptPath = join(dir, "receipt.json"), oraclePath = join(dir, "oracle.json");
    writeFileSync(receiptPath, JSON.stringify({ status: "PASS", audit_release: observation }));
    writeFileSync(oraclePath, JSON.stringify(oracle));
    const run = name => spawnSync(process.execPath, [fileURLToPath(new URL("audit-release.mjs", root)), "--receipt", receiptPath, "--oracle", oraclePath, "--output", join(dir, name)], { encoding: "utf8" });
    assert.equal(run("pass.json").status, 0);
    assert.equal(JSON.parse(readFileSync(join(dir, "pass.json"))).status, "PASS");
    writeFileSync(receiptPath, JSON.stringify({ status: "FAILED", audit_release: observation }));
    assert.equal(run("failed-browser.json").status, 1, "A saved overview from a failed browser run is not acceptance");
    assert.equal(JSON.parse(readFileSync(join(dir, "failed-browser.json"))).status, "FAIL");
    writeFileSync(receiptPath, JSON.stringify({ status: "PASS", audit_release: observation }));
    writeFileSync(oraclePath, JSON.stringify({ ...oracle, retained_event_count: "sensitive-sentinel" }));
    const failure = run("fail.json");
    assert.equal(failure.status, 1);
    assert.equal(JSON.parse(readFileSync(join(dir, "fail.json"))).status, "FAIL");
    assert.equal((failure.stdout + failure.stderr + readFileSync(join(dir, "fail.json"))).includes("sensitive-sentinel"), false);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});


test("terminal browser status is mandatory even with a valid saved overview", () => {
  const { observation, oracle } = fixture();
  assert.equal(verifyAuditReleaseReceipt({ status: "PASS", audit_release: observation }, oracle).status, "PASS");
  for (const status of ["FAILED", "INCONCLUSIVE", "RUNNING", undefined]) {
    assert.throws(() => verifyAuditReleaseReceipt({ status, audit_release: observation }, oracle));
  }
});

test("A05 checks six visible values and titles, not just exact tooltip values", () => {
  const { observation, oracle } = fixture();
  const ids = ["query_time", "scan_rows", "return_rows", "scan_bytes", "cpu_cost", "memory_cost"];
  const units = ["ns", "item", "item", "byte", "ns", "byte"];
  const values = ["2000000", "3", "4", "5000000000", "1500", "0"];
  const row = { query_id_sha256: "a".repeat(64), status: "finished", metrics: ids.map((metric_id, i) => ({ metric_id, state: "exact", value: values[i], unit: units[i] })) };
  observation.overview.body.rows = [row]; oracle.rows = structuredClone([row]);
  const dom = { query_id_sha256: row.query_id_sha256, metric_raw: values.map((value, i) => value + " " + units[i]) };
  oracle.dom_rows = [dom];
  observation.overview.rows = [{ ...dom, metric_text: ["2 ms", "3", "4", "4.657 GB", "1.5 μs", "0 B"] }];
  assert.equal(verifyAuditReleaseOracle(observation, oracle).status, "PASS");
  for (let i = 0; i < 6; i++) {
    const changed = structuredClone(observation);
    changed.overview.rows[0].metric_text[i] = "incorrect visible value";
    assert.throws(() => verifyAuditReleaseOracle(changed, oracle));
    const title = structuredClone(observation);
    title.overview.columns[i + 4] = "wrong metric title";
    assert.throws(() => verifyAuditReleaseOracle(title, oracle));
  }
  const missing = structuredClone(observation); delete missing.overview.rows[0].metric_text;
  assert.throws(() => verifyAuditReleaseOracle(missing, oracle));
});

test("A05 compares each row's statement prefix hash with the independent source, never the text", async () => {
  const { observation, oracle } = fixture();
  const statement = { state: "text", sha256: "c".repeat(64), truncated: false };
  const row = { query_id_sha256: "a".repeat(64), status: "finished", metrics: [], statement_sample: statement };
  observation.overview.body.rows = [row]; oracle.rows = structuredClone([row]);
  oracle.dom_rows = [{ query_id_sha256: row.query_id_sha256, metric_raw: [] }];
  observation.overview.rows = [{ ...oracle.dom_rows[0], metric_text: [] }];
  assert.equal(verifyAuditReleaseOracle(observation, oracle).status, "PASS");
  for (const drift of [{ ...statement, sha256: "d".repeat(64) }, { ...statement, truncated: true }, { state: "sql_null" }, { state: "unavailable", reason: "not_read" }]) {
    assert.throws(() => verifyAuditReleaseOracle(observation, { ...oracle, rows: [{ ...row, statement_sample: drift }] }));
  }
  // The observer keeps only an exact-case hash of the returned prefix and the cell it implies.
  const ids = ["query_time", "scan_rows", "return_rows", "scan_bytes", "cpu_cost", "memory_cost"];
  const metrics = ids.map(metric_id => ({ metric_id, state: "exact", canonical_number: { value: "1" }, canonical_unit: "item" }));
  const rows = [
    { query: { query_id: "ABC", status: "finished" }, metrics, statement_sample: { state: "text", value: "SELECT Secret_Stmt", truncated: true } },
    { query: { query_id: "DEF", status: "finished" }, metrics, statement_sample: { state: "sql_null" } },
  ];
  const body = { scope, coverage: { retained_event_count: "2", error_count: "0", statement_filter: statementFilter }, primary: { state: "occurrences", top: {}, rows } };
  const window = { fetch: async () => new Response(JSON.stringify(body)) };
  vm.runInNewContext(auditReleaseObserverSource(), { window, URL, Request, location: { href: "http://127.0.0.1/" }, crypto: webcrypto, TextEncoder, performance });
  await window.fetch("/api/v1/connections/x/audit/overview", { body: JSON.stringify({ request: { primary: { sort: "query_time_desc" } } }) });
  // The vm realm has its own prototypes; compare plain JSON.
  const record = JSON.parse(JSON.stringify(window.__auditRelease.records[0]));
  const sha = value => createHash("sha256").update(value).digest("hex");
  assert.deepEqual(record.rows.map(item => item.statement_sample), [{ state: "text", sha256: sha("SELECT Secret_Stmt"), truncated: true }, { state: "sql_null" }]);
  assert.deepEqual(record.statement_cells, [{ title_sha256: sha("SELECT Secret_Stmt…") }, { tag: true }]);
  assert.equal(JSON.stringify(record).includes("Secret_Stmt"), false);
});

test("F05 requires one exact real family with two retained identities and typed missing Profile", async () => {
  const { verifyMissingProfileFacts } = await import('./audit-release.mjs');
  const digest = 'a'.repeat(32), id = 'b'.repeat(64);
  const facts = { scope: { user:'workload_user', database:'e2e_fixture', native_digest:digest, window:{ start_seconds:1, end_seconds:3601 } },
    population_count:'1', states:['selected','selected','unavailable'], representative_hashes:[id,id], reason:'no_match_in_bounded_profile_evidence' };
  assert.doesNotThrow(() => verifyMissingProfileFacts(facts,digest,id));
  for (const patch of [{ population_count:'0' }, { population_count:'2' }, { states:['selected','selected','selected'] },
    { representative_hashes:[id,'c'.repeat(64)] }, { reason:'empty_cohort' }, { scope:{...facts.scope,user:'doctor_reader'} },
    { scope:{...facts.scope,native_digest:'c'.repeat(32)} }, { scope:{...facts.scope,window:{start_seconds:1,end_seconds:7201}} }])
    assert.throws(() => verifyMissingProfileFacts({...facts,...patch},digest,id));
});

test("F05 workload is opt-in one-shot with session-only profile gates and bounded source proof", () => {
  const source=readFileSync(new URL('../../dev/starrocks-3.5.14/run-fixture-workload.ps1',import.meta.url),'utf8');
  assert.match(source,/\[switch\]\$AuditReleaseAcceptance/);
  assert.equal((source.match(/SELECT payload, MIN\(id\) AS f05_min_id/g)??[]).length,1);
  assert.match(source,/SET enable_profile=false;/);
  assert.match(source,/SET big_query_profile_threshold='0s';/);
  assert.doesNotMatch(source,/SET enable_big_query_profile\s*=/i);
  assert.match(source,/\$missingRows.Count -gt 1/);
  assert.match(source,/F05 Audit digest not observed within deadline/);
  assert.match(source,/missing_profile_query_id = \$f05State.query_id/);
  const runner=readFileSync(new URL('../../scripts/e2e/run-browser-e2e.ps1',import.meta.url),'utf8');
  assert.match(runner,/\$workloadOptions.AuditReleaseAcceptance = \$true/);
  assert.match(runner,/\$env:E2E_MISSING_PROFILE_DIGEST = \$previousMissingProfileDigest/);
  assert.match(runner,/F05 real missing Profile evidence was not verified/);
});

test("F05 identities are passed only after persisted workload receipt and UI checks no implicit reads", () => {
  const workload=readFileSync(new URL('../../dev/starrocks-3.5.14/run-fixture-workload.ps1',import.meta.url),'utf8');
  assert.ok(workload.lastIndexOf('$receipt.missing_profile_query_id =') > workload.lastIndexOf('Set-Content -LiteralPath $resolvedReceiptPath'));
  const module=readFileSync(new URL('./audit-release.mjs',import.meta.url),'utf8');
  assert.match(module,/visible\.disabled === true/);
  assert.match(module,/JSON\.stringify\(counts\) === JSON\.stringify\(after\)/);
  assert.match(module,/F05 parent scope restored/);
});

test('F05 selects the family by its full digest accessible name, never its shortened visible prefix', () => {
  const source = readFileSync(new URL('./audit-release.mjs', import.meta.url), 'utf8');
  // An exact attribute selector: a prefix or a one-character change cannot match.
  assert.match(source, /table\[aria-label="Audited query families"\] button\[aria-label="Select query family \$\{digest\}"\]/);
  assert.doesNotMatch(source, /aria-label\^="Select query family \$\{digest/);
  assert.match(source, /\.audit-family-aside \.audit-family-members-more/);
  assert.match(source, /clickButton\('Distribution and representatives'\)/);
});

test('A06 leak scans disclose counts only and distinguish empty/missing logs', async () => {
  const { scanLeakValues, scanOwnedApiLogs, fixtureLeakNeedles, captureAuditStorage } = await import('./audit-release.mjs');
  const secret = 'fixture-secret-only-in-memory';
  const needles = fixtureLeakNeedles(secret);
  assert.equal(needles.length, 6);
  assert.deepEqual(scanLeakValues(['x', secret], needles), { item_count: 2, matched_item_count: 1 });
  const dir = mkdtempSync(join(tmpdir(), 'a06-log-'));
  try {
    const paths = [join(dir, 'out'), join(dir, 'err')];
    assert.equal(scanOwnedApiLogs(paths, needles).status, 'NOT_RUN');
    paths.forEach(path => writeFileSync(path, ''));
    assert.equal(scanOwnedApiLogs(paths, needles).observation, 'empty_observed');
    writeFileSync(paths[0], needles[1]);
    const result = scanOwnedApiLogs(paths, needles);
    assert.equal(result.status, 'FAIL');
    assert.equal(JSON.stringify(result).includes(needles[1]), false);
    assert.equal(scanOwnedApiLogs(paths, []).status, 'NOT_RUN');
  } finally { rmSync(dir, { recursive: true }); }
  const previous = process.env.STARROCKS_E2E_SECRET;
  process.env.STARROCKS_E2E_SECRET = secret;
  try {
    const receipt = { audit_release: {} };
    await captureAuditStorage({ receipt, evaluate: source => vm.runInNewContext(source, { localStorage: {}, sessionStorage: {} }) });
    assert.equal(receipt.audit_release.storage.observation, 'storage_empty_observed');
    await assert.rejects(captureAuditStorage({ receipt, evaluate: source => vm.runInNewContext(source, { localStorage: { password: secret }, sessionStorage: {} }) }));
  } finally { if (previous === undefined) delete process.env.STARROCKS_E2E_SECRET; else process.env.STARROCKS_E2E_SECRET = previous; }
});

test('A06 storage scan runs after final viewport and logs scan only after owned API exit', () => {
  const browser = readFileSync(new URL('./browser-e2e.mjs', import.meta.url), 'utf8');
  assert.ok(browser.indexOf('await captureAuditStorage({ evaluate, receipt })') > browser.indexOf('await narrowViewportEvidence();'));
  assert.ok(browser.indexOf('await captureAuditStorage({ evaluate, receipt })') < browser.indexOf('receipt.status = "PASS"'));
  const runner = readFileSync(new URL('../../scripts/e2e/run-browser-e2e.ps1', import.meta.url), 'utf8');
  assert.match(runner, /\$startedApi -and -not \$KeepStartedApi -and \$startedApi.HasExited/);
  assert.ok(runner.indexOf('--scan-owned-logs') > runner.indexOf('Stop-Process -Id $startedApi.Id'));
});

test('A06 needles bind actual SQL inputs and storage scans raw or JSON-serialized secrets without escaping them', async () => {
  const { fixtureLeakNeedles, scanStorageEntries } = await import('./audit-release.mjs');
  const secret = 'base-test-secret';
  const password = 'actual"password\nwith-newline';
  const needles = fixtureLeakNeedles(secret, password);
  assert.ok(needles.includes(password));
  assert.ok(needles.includes(`sr3514-doctor_reader-${secret}`));
  const workload = readFileSync(new URL('../../dev/starrocks-3.5.14/run-fixture-workload.ps1', import.meta.url), 'utf8');
  for (const alias of ['e2e_baseline', 'e2e_candidate', 'f05_min_id']) {
    const needle = needles.find(value => value.startsWith('SELECT ') && value.includes(alias));
    assert.ok(needle && workload.includes(needle + ';'));
  }
  const browser = readFileSync(new URL('./browser-e2e.mjs', import.meta.url), 'utf8');
  const actual = browser.match(/const isolatedFixtureSql\s*=\s*"([^"]+)";/)[1];
  assert.ok(needles.includes(actual));
  for (const entry of [[password, 'x'], ['key', password], ['key', JSON.stringify({ nested: { password } })]]) {
    const result = scanStorageEntries([entry], needles);
    assert.equal(result.matched_item_count, 1);
    assert.equal(result.complete, true);
    assert.equal(JSON.stringify(result).includes(password), false);
  }
  assert.equal(scanStorageEntries([['key', 'x'.repeat(65537)]], needles).complete, false);
  let deep = { password }; for (let i = 0; i < 10; i++) deep = { child: deep };
  assert.equal(scanStorageEntries([['key', JSON.stringify(deep)]], needles).complete, false);
});

test('A06 observed storage leak remains FAIL even when another entry exceeds the scan bound', async () => {
  const { captureAuditStorage } = await import('./audit-release.mjs');
  const previous = process.env.STARROCKS_E2E_SECRET;
  process.env.STARROCKS_E2E_SECRET = 'bounded-scan-test-secret';
  try {
    const receipt = { audit_release: {} };
    await assert.rejects(captureAuditStorage({ receipt, evaluate: source => vm.runInNewContext(source, {
      localStorage: { leaked: process.env.STARROCKS_E2E_SECRET, oversized: 'x'.repeat(65537) }, sessionStorage: {},
    }) }));
    assert.equal(receipt.audit_release.storage.status, 'FAIL');
    assert.equal(receipt.audit_release.storage.local.complete, false);
    assert.equal(receipt.audit_release.storage.local.matched_item_count, 1);
  } finally { if (previous === undefined) delete process.env.STARROCKS_E2E_SECRET; else process.env.STARROCKS_E2E_SECRET = previous; }
});


test("current display headers retain exact source metric identity and reject column drift", () => {
  const { observation, oracle } = fixture();
  assert.equal(verifyAuditReleaseOracle(observation, oracle).status, "PASS");
  for (const field of ["columns", "column_titles"]) {
    const changed = structuredClone(observation);
    [changed.overview[field][4], changed.overview[field][5]] = [changed.overview[field][5], changed.overview[field][4]];
    assert.throws(() => verifyAuditReleaseOracle(changed, oracle));
  }
  const ui = readFileSync(new URL("../../frontend/src/features/audit/AuditWorkloadWorkspace.tsx", import.meta.url), "utf8");
  for (let index = 4; index < 10; index++) {
    assert.ok(ui.includes(`<th title="${observation.overview.column_titles[index]}">{text("${observation.overview.columns[index]}"`));
  }
});
