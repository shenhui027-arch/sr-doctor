// Task54: real transport only. The controlled SQL is owned by the external
// fixed-Docker fixture; this harness never submits or repeats it.
import { existsSync, lstatSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { basename, isAbsolute, join } from "node:path";
const handshakeSchema = "task54-running-handshake/v1";
const uuidPattern = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i;

export function resolveRunningProfileOptions(enabled, project, queryId, conflictingMode = false, handshakeDir) {
  if (!enabled) return null;
  if (project !== "connected-sql-docker" || conflictingMode)
    throw new Error("--running-profile requires the exclusive connected-sql-docker real transport path.");
  if (handshakeDir) {
    if (queryId || !isAbsolute(handshakeDir) || !/^task54-running-[0-9a-f]{32}$/.test(basename(handshakeDir)))
      throw new Error("Running handshake requires an exclusive absolute run-specific directory.");
    return Object.freeze({ queryId: null, handshakeDir, transport: "real_loopback_api" });
  }
  if (!uuidPattern.test(queryId ?? ""))
    throw new Error("--running-profile requires E2E_RUNNING_QUERY_ID from the external fixed-Docker fixture.");
  return Object.freeze({ queryId, transport: "real_loopback_api" });
}

export function validateRunningIdentity(value, readyAt, now = Date.now()) {
  if (!value || Object.keys(value).sort().join(",") !== "observed_at_utc,query_id,schema"
    || value.schema !== handshakeSchema || !uuidPattern.test(value.query_id ?? "")
    || typeof value.observed_at_utc !== "string" || !Number.isFinite(Date.parse(value.observed_at_utc))
    || Date.parse(value.observed_at_utc) < Date.parse(readyAt) || Date.parse(value.observed_at_utc) > now + 5000)
    throw new Error("Invalid or stale Running fixture identity.");
  return value.query_id;
}

export async function awaitRunningIdentity(directory, checkZero, { now = Date.now, sleep = ms => new Promise(resolve => setTimeout(resolve, ms)) } = {}) {
  const stat = lstatSync(directory);
  if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error("Running handshake directory must be an ordinary directory.");
  const ready = join(directory, "ready.json"), identity = join(directory, "identity.json"), temporary = join(directory, "ready.json.tmp");
  if (existsSync(ready) || existsSync(identity)) throw new Error("Running handshake is single-use; existing protocol files rejected.");
  await checkZero();
  const readyAt = new Date(now()).toISOString();
  writeFileSync(temporary, JSON.stringify({ schema: handshakeSchema, ready_at_utc: readyAt }), { flag: "wx" });
  renameSync(temporary, ready);
  const deadline = now() + 45000;
  while (now() < deadline) {
    await checkZero();
    if (existsSync(identity)) {
      const identityStat = lstatSync(identity);
      if (!identityStat.isFile() || identityStat.isSymbolicLink() || identityStat.size > 4096) throw new Error("Invalid Running identity file.");
      return validateRunningIdentity(JSON.parse(readFileSync(identity, "utf8")), readyAt, now());
    }
    await sleep(100);
  }
  throw new Error("INCONCLUSIVE: Running fixture identity handshake timed out after 45 seconds.");
}

export function exactRunningCandidateIndex(ids, queryId) {
  const matches = ids.flatMap((id, index) => typeof id === "string" && id.toLowerCase() === queryId.toLowerCase() ? [index] : []);
  if (matches.length !== 1) throw new Error("Expected exactly one external Running candidate; absence is INCONCLUSIVE, not Finished.");
  return matches[0];
}

export function runningOfficialResponseMatches(response, owner) {
  return Boolean(owner && response?.kind === "analyze_profile_text"
    && response.query?.connection_id === owner.connection_id
    && response.query?.query_id === owner.query_id
    && response.query?.discovery_generation === owner.discovery_generation
    && response.identity?.kind === "discovery"
    && Object.keys(response.identity).length === 2
    && response.identity.discovery_generation === owner.discovery_generation
    && Array.isArray(response.node_ids) && response.node_ids.length === 0);
}

export function assertRunningCounts(actual, list, snapshot, official) {
  if (actual.list !== list || actual.snapshot !== snapshot || actual.official !== official || actual.forbidden !== 0)
    throw new Error("Running route counts differ from explicit gestures or a forbidden fallback was requested.");
}

// Executed in the existing project CDP harness. Only booleans, counts and HTTP
// status escape this observer; identity values and response bodies stay local.
export function runningObserverSource(queryId) {
  return `(() => {
    let target = ${JSON.stringify(queryId?.toLowerCase() ?? null)};
    const responseMatches = ${runningOfficialResponseMatches.toString()};
    const original = window.fetch;
    const records = [];
    let current = null;
    let previousGeneration = null;
    const classify = path => path.endsWith('/profiles/running-candidates') ? 'list'
      : path.endsWith('/profiles/running-snapshot') ? 'snapshot'
      : path.endsWith('/profiles/official-analysis') ? 'official'
      : /\\/(runtime-profile|explain|profiles\\/fetch)$/.test(path) ? 'forbidden' : null;
    window.__doctorE2eRunning = { records, bind: value => {
      if (target !== null || records.length !== 0 || !/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(value)) throw new Error('Running identity binding is single-use and requires zero requests.');
      target = value.toLowerCase();
    }, restore: () => { window.fetch = original; } };
    window.fetch = async (input, init) => {
      const kind = classify(new URL(typeof input === 'string' ? input : input.url, location.href).pathname);
      if (!kind) return original(input, init);
      const body = JSON.parse(init?.body ?? '{}');
      const owner = current ? Object.freeze({ connection_id: current.connection_id, query_id: current.query_id, discovery_generation: current.discovery_generation }) : null;
      const row = { kind, status: null, settled: false,
        identity_matches: kind === 'list' || Boolean(owner && body.connection_id === owner.connection_id && body.query_id === owner.query_id &&
          (kind === 'snapshot' ? body.discovery_generation === owner.discovery_generation :
            body.identity?.kind === 'discovery' && body.identity.discovery_generation === owner.discovery_generation && Array.isArray(body.node_ids) && body.node_ids.length === 0)) };
      records.push(row);
      try {
        const response = await original(input, init);
        row.status = response.status;
        if (kind === 'list' && response.ok) {
          const data = await response.clone().json();
          const candidates = data.candidates.filter(candidate => typeof candidate.query_id === 'string' && candidate.query_id.toLowerCase() === target);
          current = candidates.length === 1 ? candidates[0] : null;
          row.generation_advanced = previousGeneration === null || data.discovery_generation > previousGeneration;
          previousGeneration = data.discovery_generation;
          row.exact_candidate = Boolean(current && current.connection_id === body.connection_id && current.discovery_source === 'profile_list' && current.discovery_generation === data.discovery_generation && /^running$/i.test(current.status));
        }
        if (kind === 'official' && response.ok) row.response_identity_matches = responseMatches(await response.clone().json(), owner);
        return response;
      } finally { row.settled = true; }
    };
  })()`;
}

export async function runningProfileCoverage({ options, evidence, evaluate, clickButton, clickElement, waitFor, capture, assert }) {
  const telemetry = () => evaluate(`(() => {
    const rows = window.__doctorE2eRunning.records;
    return { counts: Object.fromEntries(['list', 'snapshot', 'official', 'forbidden'].map(kind => [kind, rows.filter(row => row.kind === kind).length])), rows };
  })()`);
  const check = async (list, snapshot, official) => {
    const data = await telemetry();
    assertRunningCounts(data.counts, list, snapshot, official);
    assert(data.rows.every(row => row.identity_matches), "Running request identity must match the current exact candidate.");
    assert(data.rows.every(row => row.kind !== "official" || !row.settled || row.status !== 200 || row.response_identity_matches === true), "Official response identity must match the frozen request owner.");
    return data;
  };
  const settle = async (kind, count = 1) => waitFor(async () => {
    const rows = (await telemetry()).rows.filter(row => row.kind === kind);
    return rows.length >= count && rows.every(row => row.settled);
  }, `bounded ${kind} response`);
  try {
    await clickButton("Running now");
    await waitFor(() => evaluate("Boolean(document.querySelector('.running-profile-entry'))"), "Running entry DOM");
    await check(0, 0, 0);
    await capture("running-r0-unread", { redactSql: true });
    let queryId = options.queryId;
    if (options.handshakeDir) {
      evidence.fixture_identity_temporarily_persisted = true;
      evidence.fixture_identity_cleanup_owner = "external_coordinator";
      queryId = await awaitRunningIdentity(options.handshakeDir, () => check(0, 0, 0));
      await evaluate(`window.__doctorE2eRunning.bind(${JSON.stringify(queryId)})`);
      await check(0, 0, 0);
    }
    await clickButton("Read running queries");
    await settle("list");
    const first = await check(1, 0, 0);
    assert(first.rows[0].status === 200 && first.rows[0].exact_candidate, "External Running candidate missing: INCONCLUSIVE; no SQL rerun or list retry.");
    await waitFor(() => evaluate("Boolean(document.querySelector('.running-profile-candidate'))"), "Running candidate DOM");
    const ids = await evaluate("Array.from(document.querySelectorAll('.running-profile-candidate')).map(row => row.querySelector('code')?.textContent)");
    const index = exactRunningCandidateIndex(ids, queryId);
    await clickElement(`document.querySelectorAll('.running-profile-candidate')[${index}]`, "Exact Running candidate");
    await check(1, 0, 0);
    await capture("running-r2-selected", { redactSql: true });
    // Official first proves it has no snapshot/report prerequisite.
    await clickButton("Read official analysis");
    await settle("official");
    await check(1, 0, 1);
    await waitFor(() => evaluate("Boolean(document.querySelector('.running-profile-official .official-profile-success'))"), "successful official DOM without snapshot prerequisite");
    await clickButton("Read Profile snapshot");
    await settle("snapshot");
    const read = await check(1, 1, 1);
    assert(read.rows.every(row => row.status === 200), "Real Running reads did not both succeed; retain typed failure evidence, not a mock PASS.");
    await waitFor(() => evaluate("Boolean(document.querySelector('.running-profile-state-success'))"), "successful snapshot DOM");
    await capture("running-r3-independent-results", { redactSql: true });
    await clickButton("Read running queries");
    await settle("list", 2);
    const refreshed = await check(2, 1, 1);
    assert(refreshed.rows.at(-1).status === 200 && refreshed.rows.at(-1).generation_advanced, "Manual refresh must return a newer discovery generation.");
    await waitFor(() => evaluate("Boolean(document.querySelector('.running-profile-retained'))"), "retained stale material DOM");
    assert(await evaluate("Boolean(document.querySelector('.running-profile-retained .official-profile-success'))"), "Successful official material must remain visible after refresh.");
    assert(await evaluate("document.querySelectorAll('.running-profile-candidate[aria-pressed=true]').length === 0 && document.querySelectorAll('.running-profile-actions button:not(:disabled)').length === 0"), "Refresh must invalidate selected live actions.");
    await capture("running-r4-retained-stale", { redactSql: true });
    // Retained success now reuses SnapshotState; scope its ordinary read-only
    // button rather than matching any live/hidden snapshot button globally.
    const retainedDoctor = "Array.from(document.querySelectorAll('.running-profile-retained button')).filter(button => button.textContent.trim() === 'Open diagnosis' && !button.disabled)";
    assert(await evaluate(`${retainedDoctor}.length === 1`), "Exactly one retained read-only Doctor action is required.");
    await clickElement(`${retainedDoctor}[0]`, "Retained read-only Doctor action");
    await waitFor(() => evaluate("Boolean(document.querySelector('.workspace-doctor'))"), "retained Doctor DOM");
    assert(await evaluate("!document.querySelector('.workspace-doctor .official-profile-open, .workspace-doctor .official-profile-dialog')"), "Retained Doctor must not expose live official authority.");
    await check(2, 1, 1);
    await capture("running-r4-retained-doctor", { redactSql: true });
    await clickButton("Queries");
    await waitFor(() => evaluate("Boolean(document.querySelector('.running-profile-retained')?.getClientRects().length)"), "retained Running materials after Doctor round trip");
    const final = await check(2, 1, 1);
    return { evidence_tier: "development_real_loopback_api", target_sql_executed_by_harness: 0,
      fixture_identity_temporarily_persisted: Boolean(options.handshakeDir), fixture_identity_cleanup_owner: options.handshakeDir ? "external_coordinator" : null,
      external_fixture_identity_exact: true, official_before_snapshot: true, retained_stale: true, retained_doctor_readonly_no_io: true,
      counts: final.counts, requests: final.rows };
  } finally {
    Object.assign(evidence, await telemetry());
    await evaluate("window.__doctorE2eRunning?.restore()");
  }
}
