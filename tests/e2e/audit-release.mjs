import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { join, isAbsolute } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import assert from "node:assert/strict";
import { stripTypeScriptTypes } from "node:module";

// Reuse the product's pure display contract, not a second unit formatter.
// This source harness runs on the same Node 24 toolchain as the browser coordinator.
const moduleUrl = source => "data:text/javascript;base64," + Buffer.from(stripTypeScriptTypes(source)).toString("base64");
const durationUrl = moduleUrl(readFileSync(new URL("../../frontend/src/lib/duration-display.ts", import.meta.url), "utf8"));
const displaySource = readFileSync(new URL("../../frontend/src/features/audit/audit-display.ts", import.meta.url), "utf8");
const { metricText } = await import(moduleUrl(displaySource.replace('"../../lib/duration-display"', JSON.stringify(durationUrl))));
const metricColumns = ["queryTime", "scanRows", "returnRows", "scanBytes", "cpuCost", "memoryCost"];
const metricColumnLabels = ["Query duration", "Scanned rows", "Returned rows", "Scanned bytes", "CPU time", "Audit memory"];

export function fixtureLeakNeedles(secret = process.env.STARROCKS_E2E_SECRET, password = process.env.E2E_DB_PASSWORD ?? process.env.STARROCKS_PASSWORD ?? (secret ? `sr3514-doctor_reader-${secret}` : undefined)) {
  if (!secret) return [];
  const source = readFileSync(new URL('../../dev/starrocks-3.5.14/run-fixture-workload.ps1', import.meta.url), 'utf8');
  const sql = [...source.matchAll(/SELECT (?:bucket, COUNT\(\*\) AS e2e_(?:baseline|candidate)|payload, MIN\(id\) AS f05_min_id)[^;]+;/g)].map(match => match[0].slice(0, -1));
  if (sql.length !== 3) return [];
  const browser = readFileSync(new URL('./browser-e2e.mjs', import.meta.url), 'utf8');
  const runtimeSql = browser.match(/const isolatedFixtureSql\s*=\s*"([^"]+)";/)?.[1];
  if (!password || !runtimeSql) return [];
  return [...new Set([secret, password, process.env.STARROCKS_PASSWORD, `sr3514-doctor_reader-${secret}`, ...sql, runtimeSql].filter(Boolean))];
}

export function scanLeakValues(values, needles) {
  return { item_count: values.length, matched_item_count: values.filter(value => needles.some(needle => String(value).includes(needle))).length };
}

export function scanStorageEntries(entries, needles) {
  let matched = 0, complete = true, nodes = 0;
  if (entries.length > 1000) complete = false;
  for (const entry of entries.slice(0, 1000)) {
    let hit = false;
    const visit = (value, depth) => {
      if (++nodes > 10000 || depth > 8) { complete = false; return; }
      if (typeof value === 'string') {
        if (value.length > 65536) { complete = false; return; }
        if (needles.some(needle => value.includes(needle))) hit = true;
      } else if (value && typeof value === 'object') {
        for (const [key, child] of Object.entries(value)) { visit(key, depth + 1); visit(child, depth + 1); }
      }
    };
    for (const value of entry) {
      visit(value, 0);
      if (typeof value === 'string' && value.length <= 65536) {
        try { visit(JSON.parse(value), 0); } catch { /* Plain storage text is already scanned. */ }
      }
    }
    if (hit) matched++;
  }
  return { item_count: entries.length, matched_item_count: matched, complete };
}

export async function captureAuditStorage({ evaluate, receipt }) {
  const needles = fixtureLeakNeedles();
  if (!needles.length) { receipt.audit_release.storage = { status: 'NOT_RUN', reason: 'needles_missing' }; return; }
  const result = await evaluate(`(() => {
    const needles = ${JSON.stringify(needles)};
    const scan = ${scanStorageEntries.toString()};
    return { local: scan(Object.entries(localStorage), needles), session: scan(Object.entries(sessionStorage), needles) };
  })()`);
  const leaked = result.local.matched_item_count + result.session.matched_item_count > 0;
  if (!leaked && (!result.local.complete || !result.session.complete)) {
    receipt.audit_release.storage = { status: 'NOT_RUN', reason: 'bounded_scan_incomplete', ...result };
    return;
  }
  receipt.audit_release.storage = { status: leaked ? 'FAIL' : 'PASS', scope: 'current_product_origin_local_and_session_storage', observation: result.local.item_count + result.session.item_count === 0 ? 'storage_empty_observed' : 'entries_scanned', ...result, database_cancel_resources: 'NOT_RUN' };
  assert.equal(leaked, false, 'Fixture material found in browser storage.');
}

export function scanOwnedApiLogs(paths, needles) {
  if (!needles.length || paths.length !== 2 || !paths.every(existsSync)) return { status: 'NOT_RUN', reason: 'needles_or_owned_logs_missing' };
  const files = paths.map(path => { const bytes = readFileSync(path); return { exists: true, byte_count: bytes.length, ...scanLeakValues([bytes.toString('utf8')], needles) }; });
  const leaked = files.some(file => file.matched_item_count > 0);
  return { status: leaked ? 'FAIL' : 'PASS', observation: files.every(file => file.byte_count === 0) ? 'empty_observed' : 'bytes_scanned', files, database_cancel_resources: 'NOT_RUN' };
}

// Scan the actual batch before projecting it; never retain the offending value.
export function containsAuditSql(value) {
  if (!value || typeof value !== "object") return false;
  return Object.entries(value).some(([key, child]) =>
    /^(sql|stmt|statement|sql_text|password|credential)$/i.test(key) || containsAuditSql(child));
}

export function auditReleaseObserverSource() {
  return String.raw`(() => {
    if (window.__auditRelease) throw new Error('Audit release observer already installed');
    const records = []; const previous = window.fetch;
    const leaks = ${containsAuditSql.toString()};
    const hash = async value => Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(value.toLowerCase())))).map(v => v.toString(16).padStart(2,'0')).join('');
    // Statement text is case-sensitive: hash it exactly and keep only the hash.
    const exactHash = async value => Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(value)))).map(v => v.toString(16).padStart(2,'0')).join('');
    const statementFact = async sample => sample?.state === 'text' ? { state: 'text', sha256: await exactHash(sample.value), truncated: sample.truncated }
      : sample?.state === 'unavailable' ? { state: 'unavailable', reason: sample.reason } : { state: sample?.state ?? 'absent' };
    window.__auditRelease = { records };
    window.fetch = async (input, init) => {
      const path = new URL(input instanceof Request ? input.url : String(input), location.href).pathname;
      if (!/\/audit\/(overview|occurrences|families|family-detail)$/.test(path)) return previous(input, init);
      const started = performance.now();
      const startedEpochMs = Date.now();
      const rawResourceRequest = JSON.parse(init.body).request;
      const resourceRequest = rawResourceRequest ? {
        scope: rawResourceRequest.scope ? Object.fromEntries(['window','state','user','catalog','database','resource_group','native_digest','statement_filter'].map(key => [key, rawResourceRequest.scope[key] ?? null])) : null,
        primary: rawResourceRequest.primary ? { state: rawResourceRequest.primary.state, sort: rawResourceRequest.primary.sort, limit: rawResourceRequest.primary.limit } : null,
        family_top_sort: rawResourceRequest.family_top_sort,
      } : null;
      const requestScope = JSON.parse(init.body).request?.scope ?? JSON.parse(init.body).scope;
      const requestWindow = requestScope?.window;
      const record = { started_epoch_ms: startedEpochMs, ended_epoch_ms: null, resource_request: resourceRequest, route: path.split('/').at(-1), duration_ms: null, status: null, aborted: false,
        window_seconds: requestWindow ? requestWindow.end_seconds-requestWindow.start_seconds : null, sql_leak: false };
      let response;
      try { response = await previous(input, init); }
      catch (error) { record.aborted = error?.name === 'AbortError'; record.status = 0; record.duration_ms = Math.round(performance.now()-started); records.push(record); throw error; }
      record.ended_epoch_ms = Date.now(); record.status = response.status; record.duration_ms = Math.round(performance.now()-started);
      if (response.ok) {
        const body = await response.clone().json();
        const primary = body.primary ?? body;
        record.sql_leak = leaks(body);
        if (record.route === 'family-detail') {
          record.missing_profile = { scope: body.scope, population_count: body.distribution?.population_count,
            states: ['recent','slowest','profile_known'].map(role => body.representatives?.[role]?.state),
            reason: body.representatives?.profile_known?.reason,
            representative_hashes: await Promise.all(['recent','slowest'].map(async role => {
              const id = body.representatives?.[role]?.occurrence?.query?.query_id;
              return id ? hash(id) : null;
            })) };
        }
        if (record.route === 'overview' && primary.state === 'occurrences') {
          record.sort = JSON.parse(init.body).request.primary.sort;
          record.scope = body.scope;
          record.audit_timestamp_time_zone = body.audit_timestamp_time_zone;
          record.top = primary.top;
          record.retained_event_count = body.coverage.retained_event_count;
          record.error_count = body.coverage.error_count;
          record.statement_filter = body.coverage.statement_filter;
          record.rows = await Promise.all(primary.rows.map(async row => ({ query_id_sha256: await hash(row.query.query_id), status: row.query.status,
            metrics: ['query_time','scan_rows','return_rows','scan_bytes','cpu_cost','memory_cost'].map(id => row.metrics.find(m => m.metric_id === id)).map(m => ({ metric_id:m.metric_id, state:m.state, value:m.canonical_number?.value ?? null, unit:m.canonical_unit ?? null })),
            statement_sample: await statementFact(row.statement_sample) })));
          // What the SQL cell must show for each row: its exact title, or a dashed tag.
          record.statement_cells = await Promise.all(primary.rows.map(async row => row.statement_sample?.state === 'text' && row.statement_sample.value.trim() !== ''
            ? { title_sha256: await exactHash(row.statement_sample.value + (row.statement_sample.truncated ? '…' : '')) } : { tag: true }));
        }
      }
      record.observer_overhead_ms = Math.round(performance.now()-started)-record.duration_ms;
      records.push(record);
      return response;
    };
  })()`;
}

export async function captureAuditReleaseRequests({ evaluate, assert: check, receipt }) {
  const records = await evaluate(`window.__auditRelease.records.map(({route,status,duration_ms,observer_overhead_ms,window_seconds,aborted,sql_leak}) => ({route,status,duration_ms,observer_overhead_ms,window_seconds,aborted,sql_leak}))`);
  check(records.every(row => !row.sql_leak), "Audit batch response contained a forbidden SQL/credential field.");
  check(records.filter(row => row.route !== "family-detail").every(row => row.window_seconds > 0 && row.window_seconds <= 2592000), "Audit sent an unbounded or oversized window.");
  receipt.audit_release.requests = records;
  receipt.audit_release.thirty_day_read_observed = records.some(row => row.window_seconds === 2592000);
  receipt.audit_release.timeout_cancel_retry = "NOT_EXERCISED_UNLESS_SEPARATE_FAILURE_RECEIPT";
  receipt.audit_release.timing_semantics = "duration_ms is fetch roundtrip, not database CPU/time; observer_overhead_ms includes awaited clone/JSON/hash before returning the unchanged response";
}

export async function captureAuditReleaseOverview({ evaluate, assert: check, receipt }) {
  const result = await evaluate(`(async () => {
    const body = window.__auditRelease.records.slice().reverse().find(r => r.route === 'overview' && r.rows);
    const hash = async value => Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(value.toLowerCase())))).map(v => v.toString(16).padStart(2,'0')).join('');
    const exactHash = async value => Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(value)))).map(v => v.toString(16).padStart(2,'0')).join('');
    const table = document.querySelector('table[aria-label="Audited query rankings"]');
    return { body, columns: Array.from(table?.querySelectorAll('thead th') ?? []).map(n => n.textContent.trim()),
      column_titles: Array.from(table?.querySelectorAll('thead th') ?? []).map(n => n.title),
      rows: await Promise.all(Array.from(table?.querySelectorAll('tbody tr') ?? []).map(async tr => ({
        query_id_sha256: await hash(tr.querySelector('button').getAttribute('aria-label').replace('Select audited query ','')),
        statement_cell: tr.cells[1]?.querySelector('.audit-value-tag') ? { tag: true } : { title_sha256: await exactHash(tr.cells[1]?.title ?? '') },
        metric_raw: Array.from(tr.querySelectorAll('td')).slice(4,10).map(td => td.title),
        metric_text: Array.from(tr.querySelectorAll('td')).slice(4,10).map(td => td.textContent.trim())
      }))), counts: Array.from(document.querySelectorAll('.audit-kpi .audit-kpi-value')).map(n => n.textContent.trim().replaceAll(',', '')),
      statement_filter_dom: (() => { const node = document.querySelector('.audit-board-main-head [data-audit-statement-filter]'); return node ? { state: node.getAttribute('data-audit-statement-filter'), text: node.textContent + ' ' + (node.getAttribute('title') ?? '') } : null; })() };
  })()`);
  check(result.body && !result.body.sql_leak, "Audit overview batch leaked SQL or lacked real response evidence.");
  if (receipt.project === "connected-readonly-docker") check(result.body.scope.user === "workload_user" && result.body.window_seconds === 3600, "A05 must use the explicit immutable workload-user cohort.");
  check(JSON.stringify(result.rows.map(r => r.query_id_sha256)) === JSON.stringify(result.body.rows.map(r => r.query_id_sha256)), "Audit page Top order differs from response.");
  check(result.counts[0] === result.body.retained_event_count && result.counts[4] === result.body.error_count, "Audit page full-window counts differ from response.");
  check(statementFilterDomMatches(result.statement_filter_dom, result.body.statement_filter), "Audit page hides statement-filter rows without showing the server hidden counts.");
  check(result.columns.length === 11 && result.columns[1] === "SQL" && JSON.stringify(result.columns.slice(4,10)) === JSON.stringify(metricColumnLabels) && JSON.stringify(result.column_titles.slice(4,10)) === JSON.stringify(metricColumns) && result.rows.every(r => r.metric_raw.length === 6 && r.metric_text.length === 6), "Audit page metric columns missing or reordered.");
  check(JSON.stringify(result.rows.map(r => r.statement_cell)) === JSON.stringify(result.body.statement_cells), "Audit page SQL cells differ from each row's returned statement sample.");
  receipt.audit_release = { ...receipt.audit_release, schema: "audit-release-observation/v1", transport: "real_loopback_api", oracle_status: "NOT_RUN", overview: result,
    database_timeout_resource_evidence: "NOT_RUN", logs_and_long_term_cache_scan: "NOT_RUN" };
}

export function verifyMissingProfileFacts(facts, digest, queryHash) {
  assert.equal(facts.scope.user, 'workload_user');
  assert.equal(facts.scope.database, 'e2e_fixture');
  assert.equal(facts.scope.native_digest, digest);
  assert.equal(facts.scope.window.end_seconds - facts.scope.window.start_seconds, 3600);
  assert.equal(facts.population_count, '1');
  assert.deepEqual(facts.states, ['selected', 'selected', 'unavailable']);
  assert.deepEqual(facts.representative_hashes, [queryHash, queryHash]);
  assert.equal(facts.reason, 'no_match_in_bounded_profile_evidence');
}

export async function captureMissingProfileFamily({ evaluate, check, clickButton, setInputValue, clickElement, waitFor, routeTelemetry, capture, receipt, digest, queryHash }) {
  check(/^[0-9a-f]{32}$/.test(digest), 'F05 requires the real fixture digest.');
  await clickButton('Queries');
  if (await evaluate("Boolean(document.querySelector('.audit-family-detail'))")) await clickButton('Return to parent window');
  await setInputValue('select[aria-label="Audit window"]', '1h');
  await setInputValue('select[aria-label="Audit source state"]', '');
  for (const [label, value] of [['Exact user filter', 'workload_user'], ['Database filter', 'e2e_fixture'], ['Catalog filter', ''], ['Resource group filter', '']])
    await setInputValue(`input[aria-label="${label}"]`, value);
  check(await evaluate('Boolean(document.querySelector(\'table[aria-label="Audited query families"]\'))'), 'F05 requires the current real family list.');
  const before = (await routeTelemetry('/audit/overview')).length;
  await clickButton('Apply filters');
  await waitFor(async () => (await routeTelemetry('/audit/overview')).length > before
    && await evaluate(`Boolean(document.querySelector('table[aria-label="Audited query families"] button[aria-label="Select query family ${digest}"]'))`), 'F05 exact fixture family');
  const counts = await Promise.all(['/audit/sql', '/profiles/fetch', '/runtime-profile'].map(async route => (await routeTelemetry(route)).length));
  const detailsBefore = await evaluate("window.__auditRelease.records.filter(row => row.route === 'family-detail').length");
  await clickElement(`document.querySelector('table[aria-label="Audited query families"] button[aria-label="Select query family ${digest}"]')`, 'F05 family row unavailable.');
  await waitFor(async () => await evaluate("Boolean(document.querySelector('.audit-family-aside .audit-family-members-more'))"), 'F05 family members');
  await clickElement('document.querySelector(".audit-family-aside .audit-family-members-more")', 'F05 family member-table entry unavailable.');
  await waitFor(async () => await evaluate('Boolean(document.querySelector(\'.audit-board-main table[aria-label="Audited query rankings"]\'))'), 'F05 family members table');
  await clickButton('Distribution and representatives');
  await waitFor(async () => await evaluate(`window.__auditRelease.records.filter(row => row.route === 'family-detail').length > ${detailsBefore} && Boolean(document.querySelector('.audit-family-detail'))`), 'F05 unavailable Profile detail');
  const facts = await evaluate("window.__auditRelease.records.filter(row => row.route === 'family-detail').at(-1).missing_profile");
  verifyMissingProfileFacts(facts, digest, queryHash);
  const visible = await evaluate(`(() => {
    const root = document.querySelector('.audit-family-detail');
    return { counts: root.querySelector('.audit-family-detail-counts')?.textContent,
      unavailable: root.querySelector('.audit-family-representative-profile_known p[title]')?.getAttribute('title'),
      unavailableText: root.querySelector('.audit-family-representative-profile_known')?.textContent,
      recent: Boolean(root.querySelector('.audit-family-representative-recent .audit-family-representative-id')),
      slowest: Boolean(root.querySelector('.audit-family-representative-slowest .audit-family-representative-id')),
      disabled: [...root.querySelectorAll('button')].find(button => button.textContent === 'Fetch Profile for evidence representative')?.disabled };
  })()`);
  // The backend reason code stays in the title; the visible text is its translation (v0.6-09).
  check(visible.counts?.includes('Executions 1 · with query time 1') && visible.recent && visible.slowest
    && visible.disabled === true && visible.unavailable === facts.reason
    && visible.unavailableText?.includes('No Profile found among the checked candidates') && !visible.unavailableText.includes(facts.reason), 'F05 UI lost Audit facts or enabled missing Profile.');
  const after = await Promise.all(['/audit/sql', '/profiles/fetch', '/runtime-profile'].map(async route => (await routeTelemetry(route)).length));
  check(JSON.stringify(counts) === JSON.stringify(after), 'F05 implicitly read evidence or executed SQL.');
  await capture('audit-f05-uncollected-profile-facts-retained-1440x1024');
  receipt.audit_release.missing_profile = { status: 'PASS', scenario: 'real_uncollected_profile', population_count: '1', reason: facts.reason, facts_retained: true, fetch_disabled: true, zero_implicit_reads_or_execution: true };
  await clickButton('Return to parent window');
  await waitFor(async () => await evaluate('Boolean(document.querySelector(\'table[aria-label="Audited query families"]\')) && document.querySelector(".workspace-queries .audit-workload")?.getAttribute("aria-busy") === "false"'), 'F05 parent scope restored');
}

// D2: hidden rows must stay visible as server counts, never silently dropped.
export function statementFilterDomMatches(dom, coverage) {
  if (!dom || !coverage || dom.state !== coverage.state) return false;
  if (coverage.state !== "applied") return coverage.state === "not_applied";
  return [coverage.hidden_non_query_row_count, coverage.hidden_doctor_row_count].every(count => /^(0|[1-9][0-9]*)$/.test(count) && dom.text.includes(count));
}

// Independent oracle is produced from AuditLoader source SQL, never from the DTO.
export function verifyAuditReleaseOracle(observation, oracle) {
  assert.equal(observation.schema, "audit-release-observation/v1");
  assert.equal(observation.transport, "real_loopback_api");
  assert.equal(oracle.schema, "audit-release-oracle/v1");
  assert.equal(oracle.source, "fixed_3.5.14_auditloader_independent_sql");
  assert.equal(oracle.status, "PASS");
  assert.equal(oracle.cohort_complete, true);
  const actual = observation.overview;
  assert.equal(actual.body.scope.user, "workload_user");
  assert.equal(actual.body.scope.window.end_seconds - actual.body.scope.window.start_seconds, 3600);
  assert.deepEqual(actual.body.scope, oracle.scope, "Oracle must use the identical [start,end) and filters");
  assert.equal(actual.body.sql_leak, false);
  assert.ok(["all", "exclude_doctor_and_non_query"].includes(actual.body.scope.statement_filter), "Oracle cohort must name the explicit statement filter");
  for (const key of ["sort", "audit_timestamp_time_zone", "retained_event_count", "error_count", "statement_filter", "top", "rows"]) assert.deepEqual(actual.body[key], oracle[key], `Independent oracle mismatch: ${key}`);
  assert.ok(statementFilterDomMatches(actual.statement_filter_dom, oracle.statement_filter), "Rendered statement-filter disclosure must match the independent hidden counts");
  assert.equal(actual.columns.length, 11);
  assert.equal(actual.columns[1], "SQL");
  assert.deepEqual(actual.columns.slice(4,10), metricColumnLabels, "Rendered metric labels/order must match the current table contract");
  assert.deepEqual(actual.column_titles.slice(4,10), metricColumns, "Metric source titles/order must match the frozen metric contract");
  assert.deepEqual(actual.rows.map(({ query_id_sha256, metric_raw }) => ({ query_id_sha256, metric_raw })), oracle.dom_rows, "Rendered exact metric values/Top must match independent source");
  assert.deepEqual(actual.rows.map(row => row.metric_text), oracle.rows.map(row => row.metrics.map(metric => metricText({
    state: metric.state, canonical_number: { kind: "integer", scale: null, value: metric.value }, canonical_unit: metric.unit,
  }))), "Rendered visible metric text must match the product display contract applied to independent source");
  return { status: "PASS", evidence: "real_auditloader_same_window_oracle", compared_rows: oracle.rows.length };
}

export function verifyAuditReleaseReceipt(receipt, oracle) {
  assert.equal(receipt.status, "PASS", "Browser receipt must be terminal PASS, not a partial snapshot");
  return verifyAuditReleaseOracle(receipt.audit_release, oracle);
}

if (process.argv[2] === '--scan-owned-logs') {
  console.log(JSON.stringify(scanOwnedApiLogs(process.argv.slice(3), fixtureLeakNeedles())));
} else if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const option = name => { const index = process.argv.indexOf(name); assert.ok(index >= 2 && process.argv[index + 1], `Missing ${name}`); return process.argv[index + 1]; };
  const output = option("--output");
  try {
    const receipt = JSON.parse(readFileSync(option("--receipt"), "utf8"));
    const oracle = JSON.parse(readFileSync(option("--oracle"), "utf8"));
    const result = verifyAuditReleaseReceipt(receipt, oracle);
    writeFileSync(output, JSON.stringify(result, null, 2) + "\n", { flag: "wx" });
    console.log(JSON.stringify(result));
  } catch {
    // Assertion details may include source identities; keep failure output value-free.
    writeFileSync(output, JSON.stringify({ status: "FAIL", reason: "audit_release_oracle_mismatch_or_invalid_input" }) + "\n", { flag: "wx" });
    process.exitCode = 1;
  }
}


// Independent source-resource observation; never derives expected metrics from DTOs.
export function resourceProbeInput(record, builderOutput) {
  assert.equal(record.route, 'overview'); assert.equal(record.status, 200);
  assert.equal(record.window_seconds, 2592000); assert.equal(record.sql_leak, false);
  const request = record.resource_request;
  assert.equal(request.primary.state, 'occurrences');
  assert.deepEqual(request.scope, record.scope);
  const start = Math.floor(record.started_epoch_ms / 1000);
  const end = Math.floor(record.ended_epoch_ms / 1000) + 1;
  assert.ok(Number.isSafeInteger(start) && Number.isSafeInteger(end) && start >= 0 && end > start && end - start <= 300);
  const hashes = {};
  const lines = builderOutput.trim().split(/\r?\n/);
  assert.equal(lines.length, 4);
  for (const line of lines) {
    const match = /^(coverage|primary|concurrency|family_top)\t([0-9a-f]+)$/.exec(line);
    assert.ok(match && match[2].length % 2 === 0 && !Object.hasOwn(hashes, match[1]));
    hashes[match[1]] = createHash('sha256').update(Buffer.from(match[2], 'hex')).digest('hex');
  }
  assert.equal(new Set(Object.values(hashes)).size, 4);
  return { schema: 'audit-resource-input/v1', execution_window: { start_seconds: start, end_seconds: end }, scope: request.scope,
    primary_limit: request.primary.limit, primary_sort: request.primary.sort, family_top_sort: request.family_top_sort,
    statement_hashes: hashes, audit_timestamp_time_zone: record.audit_timestamp_time_zone, attribution: 'resource_observation_only' };
}
export function resourceBuilderInput(record) {
  const r = record.resource_request;
  const hex = value => value == null ? '-' : Buffer.from(value, 'utf8').toString('hex');
  assert.ok(["all", "exclude_doctor_and_non_query"].includes(r.scope.statement_filter));
  return [r.scope.window.start_seconds, r.scope.window.end_seconds, r.primary.limit, r.primary.sort, r.family_top_sort,
    ...['state','user','catalog','database','resource_group','native_digest'].map(key => hex(r.scope[key])), r.scope.statement_filter].join('\n') + '\n';
}
export async function captureAuditResourceProbe({ evaluate, receipt, runRoot, env = process.env, execute = spawnSync }) {
  if (!env.E2E_AUDIT_RESOURCE_BUILDER) return;
  receipt.audit_release ??= {};
  receipt.audit_release.resource_probe = { status: 'INCONCLUSIVE', attribution: 'resource_observation_only', request_attribution: 'NOT_VERIFIED' };
  const resource = receipt.audit_release.resource_probe;
  let resourceStage = 'owned_identity';
  try {
    assert.equal(receipt.project, 'connected-readonly-docker');
    assert.equal(env.E2E_AUDIT_RESOURCE_OWNED, '1');
    assert.ok(isAbsolute(env.E2E_AUDIT_RESOURCE_BUILDER) && isAbsolute(env.E2E_BUILD_IDENTITY_PATH));
    const identity = JSON.parse(readFileSync(env.E2E_BUILD_IDENTITY_PATH, 'utf8').replace(/^\uFEFF/, ''));
    assert.equal(identity.runtime.api_owner, 'this_script'); assert.equal(identity.runtime.verification, 'verified');
    assert.ok(Number.isSafeInteger(identity.runtime.process_id) && identity.runtime.process_id > 0);
    resource.owned_build_identity_sha256 = createHash('sha256').update(readFileSync(env.E2E_BUILD_IDENTITY_PATH)).digest('hex');
    resourceStage = 'overview_observation';
    const record = await evaluate(`(() => { const r = window.__auditRelease.records.slice().reverse().find(r => r.route === 'overview' && r.status === 200); if (!r) return null; const {route,status,window_seconds,sql_leak,started_epoch_ms,ended_epoch_ms,resource_request,scope,audit_timestamp_time_zone} = r; return {route,status,window_seconds,sql_leak,started_epoch_ms,ended_epoch_ms,resource_request,scope,audit_timestamp_time_zone}; })()`);
    assert.ok(record);
    if (record.window_seconds !== 2592000) {
      assert.ok(Number.isSafeInteger(record.window_seconds) && record.window_seconds > 0 && record.window_seconds < 2592000);
      resource.status = 'NOT_RUN'; resource.reason = 'observed_overview_not_thirty_days';
      resource.observed_window_seconds = record.window_seconds;
      return; // Preserve the explicit 30d resource contract; never issue a replacement request.
    }
    resourceStage = 'builder_export';
    const built = execute(env.E2E_AUDIT_RESOURCE_BUILDER, [], { input: resourceBuilderInput(record), encoding: 'utf8', timeout: 5000, maxBuffer: 1048576, windowsHide: true });
    resource.process_exit = Number.isSafeInteger(built.status) ? built.status : null;
    resource.process_timeout = built.error?.code === 'ETIMEDOUT';
    assert.equal(built.status, 0); assert.ok(!built.error);
    const input = resourceProbeInput(record, built.stdout);
    built.stdout = null; built.stderr = null;
    // Close the rounded-up second before the probe's fixed-window freshness check.
    await new Promise(resolve => setTimeout(resolve, Math.max(0, input.execution_window.end_seconds * 1000 - Date.now())));
    resourceStage = 'input_write';
    const inputPath = join(runRoot, 'audit-resource-input.json');
    const outputPath = join(runRoot, 'audit-resource-observation.json');
    assert.ok(!existsSync(inputPath) && !existsSync(outputPath));
    writeFileSync(inputPath, JSON.stringify(input, null, 2), { flag: 'wx' });
    resource.input_path = inputPath; resource.input_sha256 = createHash('sha256').update(readFileSync(inputPath)).digest('hex');
    const args = ['-NoProfile','-File',fileURLToPath(new URL('../../scripts/e2e/audit-resource-probe.ps1', import.meta.url)), '-InputPath',inputPath,'-OutputPath',outputPath,'-BuilderExecutable',env.E2E_AUDIT_RESOURCE_BUILDER];
    if (env.E2E_AUDIT_RESOURCE_DOCKER) args.push('-DockerExecutable', env.E2E_AUDIT_RESOURCE_DOCKER);
    resourceStage = 'probe_process';
    resource.process_exit = null; resource.process_timeout = false;
    const observed = execute(env.E2E_AUDIT_RESOURCE_PWSH ?? 'pwsh', args, { encoding: 'utf8', timeout: 45000, maxBuffer: 65536, windowsHide: true });
    resource.process_exit = Number.isSafeInteger(observed.status) ? observed.status : null;
    resource.process_timeout = observed.error?.code === 'ETIMEDOUT';
    if (existsSync(outputPath)) { resource.path = outputPath; resource.sha256 = createHash('sha256').update(readFileSync(outputPath)).digest('hex'); }
    assert.equal(observed.status, 0); assert.ok(!observed.error);
    resourceStage = 'probe_receipt';
    const facts = JSON.parse(readFileSync(outputPath, 'utf8').replace(/^\uFEFF/, ''));
    assert.equal(facts.schema, 'audit-resource-observation/v1'); assert.equal(facts.status, 'OBSERVED');
    assert.equal(facts.attribution, 'resource_observation_only'); assert.equal(facts.request_attribution, 'NOT_VERIFIED');
    assert.equal(facts.input_sha256, resource.input_sha256); assert.equal(facts.rows.length, 4);
    resource.status = 'OBSERVED';
  } catch {
    resource.failure_stage = resourceStage;
    resource.reason = 'resource_probe_failed';
    throw new Error('audit_resource_probe_failed'); // Never serialize native stdout/stderr or SQL.
  }
}
