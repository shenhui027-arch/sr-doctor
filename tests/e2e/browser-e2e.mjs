import {
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { mkdtemp, readFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { spawn } from "node:child_process";
import { tmpdir } from "node:os";
import { createHash } from "node:crypto";
import { resolveRunningProfileOptions, runningObserverSource, runningProfileCoverage } from "./running-profile.mjs";
import { auditReleaseObserverSource, captureAuditReleaseOverview, captureAuditReleaseRequests, captureMissingProfileFamily, captureAuditStorage, captureAuditResourceProbe } from "./audit-release.mjs";
import {
  officialProfileIdentityEchoes,
  sameOfficialProfileIdentitySummary,
} from "./official-path.mjs";
const project = option("--project") ?? "offline-local";
if (
  ![
    "offline-local",
    "connected-readonly-docker",
    "connected-sql-docker",
    "connected-audit-docker",
  ].includes(project)
)
  fail(`Unknown project: ${project}`);

const officialProfileTransport =
  process.env.E2E_OFFICIAL_PROFILE_TRANSPORT ?? "real_loopback_api";
if (
  !["real_loopback_api", "controlled_mock_over_real_session"].includes(
    officialProfileTransport,
  )
)
  fail(`Unknown official Profile transport: ${officialProfileTransport}`);
const runtimeUnknownGuardTransport = resolveRuntimeUnknownGuardTransport(
  process.env.E2E_RUNTIME_UNKNOWN_GUARD_TRANSPORT,
  project,
);
const retainedProfileHashAllowlist = parseSha256Allowlist(
  process.env.E2E_RETAINED_PROFILE_HASHES,
);
const runningProfileOptions = resolveRunningProfileOptions(
  process.argv.includes("--running-profile"), project, process.env.E2E_RUNNING_QUERY_ID,
  runtimeUnknownGuardTransport !== "real_loopback_api" || officialProfileTransport !== "real_loopback_api",
  process.env.E2E_RUNNING_HANDSHAKE_DIR,
);
const baseUrl = (process.env.E2E_BASE_URL ?? "http://127.0.0.1:8000").replace(
  /\/$/,
  "",
);

// This bounded statement targets metadata provided by the isolated StarRocks
// fixture only. It is never copied from an Audit row or customer SQL.
const isolatedFixtureSql =
  "SELECT COUNT(*) AS browser_runtime_count FROM information_schema.columns";
const isolatedPlanSql =
  project === "connected-sql-docker"
    ? fixedComplexPlanSql()
    : isolatedFixtureSql;
const expectedAuditWindowPresets = Object.freeze([
  "1h",
  "12h",
  "1d",
  "3d",
  "7d",
  "30d",
]);
const auditWindowSecondsByPreset = Object.freeze({
  "1h": 3600,
  "12h": 43200,
  "1d": 86400,
  "3d": 259200,
  "7d": 604800,
  "30d": 2592000,
});
const auditFixtureWindow = resolveAuditFixtureWindow(
  process.env.E2E_AUDIT_FIXTURE_WINDOW,
  project,
  officialProfileTransport,
);
const auditOverviewRoute = "/audit/overview";
// Outside the repository: SR_DOCTOR_WORK_ROOT, or sr-doctor under the system temp directory.
const workRoot = process.env.SR_DOCTOR_WORK_ROOT ?? join(tmpdir(), "sr-doctor");
const artifactRoot = resolve(
  process.env.E2E_ARTIFACT_ROOT ?? join(workRoot, "e2e", "browser"),
);
const runRoot = join(artifactRoot, `${project}-${stamp()}-${process.pid}`);
const chromeProfileRoot = resolve(
  process.env.E2E_CHROME_PROFILE_ROOT ?? join(workRoot, "e2e", "chrome-profiles"),
);
const buildIdentity = loadBuildIdentity();
mkdirSync(runRoot, { recursive: true });
mkdirSync(chromeProfileRoot, { recursive: true });
const receipt = {
  project,
  base_url: baseUrl,
  build_identity: buildIdentity,
  started_utc: new Date().toISOString(),
  assertions: [],
  screenshots: [],
  status: "FAILED",
};
let chrome;
let cdp;
let chromeProfile;
try {
  await requireApi();
  chromeProfile = await mkdtemp(join(chromeProfileRoot, "browser-"));
  chrome = await launchChrome(chromeProfile);
  cdp = await connectCdp(chrome.debuggerBase);
  await cdp.send("Page.enable");
  await cdp.send("Runtime.enable");
  await cdp.send("Emulation.setDeviceMetricsOverride", {
    width: 1440,
    height: 1024,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await navigate(baseUrl);
  await installRouteTelemetry();
  if (runningProfileOptions) await evaluate(runningObserverSource(runningProfileOptions.queryId));
  await keyboardLocaleAndWorkspaceCoverage();
  await connectionDialogAccessibilityCoverage();
  await offlineImportCoverage();
  if (project === "connected-readonly-docker")
    await connectedReadonlyCoverage();
  if (project === "connected-audit-docker") {
    await connectFixtureCoverage();
    await auditWorkloadCoverage();
    assert(
      receipt.audit_families?.drill_state ===
        "exact_native_digest_detail_loaded",
      "Focused Audit E2E requires an actual exact-family detail, not a skipped drill.",
    );
  }
  if (project === "connected-sql-docker") {
    if (runtimeUnknownGuardTransport === "controlled_mock_over_real_session") {
      await runtimeUnknownGuardCoverage();
    } else if (runningProfileOptions) {
      await connectFixtureCoverage();
      await clickButton("Queries");
      receipt.running_profile = { evidence_tier: "development_real_loopback_api", status: "INCOMPLETE" };
      receipt.running_profile = await runningProfileCoverage({
        options: runningProfileOptions, evidence: receipt.running_profile, evaluate, clickButton, clickElement, waitFor, capture, assert,
      });
    } else {
      await connectFixtureCoverage();
      await clickButton("Queries");
      await topologyAndRuntimeCoverage();
    }
  }
  await capture("primary-1440x1024", { redactSql: Boolean(runningProfileOptions) });
  await narrowViewportEvidence();
  if (project === "connected-readonly-docker" && process.env.E2E_MISSING_PROFILE_DIGEST) await captureAuditStorage({ evaluate, receipt });
  receipt.status = "PASS";
} catch (error) {
  receipt.error = error instanceof Error ? error.message : String(error);
  if (cdp) {
    try {
      receipt.failure_routes = {
        explain: await routeTelemetry("/explain"),
        runtime: await routeTelemetry("/runtime-profile"),
        queries: await routeTelemetry("/queries"),
        audit_overview: await routeTelemetry("/audit/overview"),
      };
      await capture("failure-1440x1024", {
        redactSql: Boolean(runningProfileOptions),
      });
    } catch {
      receipt.failure_capture_unavailable = true;
    }
  }
} finally {
  receipt.completed_utc = new Date().toISOString();
  writeFileSync(
    join(runRoot, "browser-e2e-receipt.json"),
    `${JSON.stringify(receipt, null, 2)}\n`,
    "utf8",
  );
  await cdp?.close();
  await closeChrome(chrome);
  if (chromeProfile) await removeOwnedChromeProfile(chromeProfile);
}
console.log(
  JSON.stringify({
    project,
    status: receipt.status,
    receipt: join(runRoot, "browser-e2e-receipt.json"),
    assertions: receipt.assertions.length,
    screenshots: receipt.screenshots.length,
  }),
);
if (receipt.status !== "PASS") process.exitCode = 1;
async function requireApi() {
  const response = await fetch(`${baseUrl}/api/v1/capabilities`);
  if (!response.ok)
    fail(`Local API is unavailable at ${baseUrl} (HTTP ${response.status}).`);
  receipt.assertions.push("local_api_available");
}
function fixedComplexPlanSql() {
  const bytes = readFileSync(
    resolve("fixtures/synthetic/complex-query.sql"),
  );
  assert(
    createHash("sha256").update(bytes).digest("hex") ===
      "517b9da3b06e8c7e41864b6187793d46267f110b1c4b5b21b12b555b8eeef024",
    "Complex SQL fixture hash mismatch.",
  );
  return bytes.toString("utf8");
}
async function keyboardLocaleAndWorkspaceCoverage() {
  receipt.keyboard_trace = [];
  const chinese = await tabUntil(
    (active) => active?.ariaLabel === "Switch to Chinese",
    "language switch",
  );
  await key("Enter");
  await waitFor(() => has('button[aria-label="切换至英文"][aria-pressed="false"]'), "Chinese locale");
  receipt.assertions.push(`keyboard_tab_enter_locale:${chinese.ariaLabel}`);
  await tabUntil(
    (active) => active?.ariaLabel === "切换至英文",
    "English language switch",
  );
  await key("Enter");
  await waitFor(
    () => has('button[aria-label="Switch to Chinese"][aria-pressed="false"]'),
    "English locale",
  );
  receipt.assertions.push("keyboard_tab_enter_locale:English");
  // First run (design board 05): Doctor and Recommendations stay disabled until a
  // Profile exists, so keyboard navigation reaches the next enabled workspace.
  assert(
    await evaluate(`Array.from(document.querySelectorAll(".workspace-nav button")).filter((button) => button.disabled).map((button) => button.textContent.trim()).join("|") === "Doctor|Recommendations"`),
    "First run must disable exactly Doctor and Recommendations.",
  );
  await tabUntil((active) => active?.text === "Compare", "Compare workspace");
  await key("Enter");
  await waitFor(() => has(".workspace-compare"), "Compare workspace navigation");
  receipt.assertions.push("keyboard_tab_enter_workspace:Compare_first_run_doctor_disabled");
}
async function connectionDialogAccessibilityCoverage() {
  await clickButton("Connect");
  await waitFor(() => has('[role="dialog"]'), "connection dialog");
  // Hosts are typed freely (no preset address since the free connection
  // form), so the first field, the host, starts focused and empty.
  const initialFocus = await activeSummary();
  assert(
    initialFocus?.tag === "INPUT" && initialFocus?.value === "" &&
      (await evaluate(`document.activeElement === document.querySelector('[role="dialog"] input')`)),
    "Connection dialog did not focus host input.",
  );
  await key("Escape");
  await waitFor(
    () => has('[role="dialog"]').then((present) => !present),
    "connection dialog close",
  );
  const restored = await activeSummary();
  assert(
    restored?.ariaLabel === "Connect",
    "Connection dialog did not restore focus to its caller.",
  );
  receipt.assertions.push("connection_dialog_initial_focus_escape_restore");
}
async function offlineImportCoverage() {
  // Without a session, Queries is the first-run page: offline import and the
  // read-only connection side by side, with no demo cluster data.
  await clickButton("Queries");
  await waitFor(() => has(".workspace-queries .first-run"), "first-run page");
  assert(
    (await has(".first-run .import-panel")) && (await has(".first-run .connection-form")) &&
      !(await evaluate(`/Demo|Mock cluster/.test(document.querySelector('.first-run').innerText)`)),
    "First-run page must offer import and connection without demo data.",
  );
  receipt.assertions.push("first_run_import_and_connect_without_demo_data");
  await capture("first-run-1440x1024");
  // The first-run card is the importer; the header import button returns once a session or report exists.
  assert(!(await has(".app-header .button.accent")), "First run must not duplicate the import entry in the header.");
  const complexProfile = process.env.E2E_COMPLEX_PROFILE_PATH;
  const fixturePath = resolve(
    complexProfile ??
      "fixtures/runtime/3.5.14/docker_information_schema_columns.profile.txt",
  );
  const profile = readFileSync(fixturePath, "utf8");
  if (complexProfile) {
    const hash = createHash("sha256")
      .update(readFileSync(fixturePath))
      .digest("hex");
    assert(
      hash ===
        "9538497e592e029e4439f3272a0d05fb81fbe682d488ccd703b3b97d92d1d2ce",
      "Complex retained Profile identity mismatch.",
    );
    receipt.complex_profile = {
      sha256: hash,
      source: "retained_sr3514_docker_profile",
      live_database_query: false,
    };
  }
  await setInputValue(".first-run .import-panel textarea", profile);
  await wait(100); // Let React commit the controlled input before the submit click.
  receipt.offline_input = await evaluate(
    `({ mode: 'paste', byte_length: document.querySelector('.first-run .import-panel textarea')?.value.length ?? 0 })`,
  );
  const analysesBefore = (await projectionTelemetry("/analyses")).length;
  await clickButton("Analyze Profile");
  await waitFor(
    () => has(".workspace-doctor .doctor-summary"),
    "offline import Doctor response",
  );
  const projection = await projectionAfterRoute(
    "/analyses",
    analysesBefore,
    "Offline import",
  );
  receipt.offline_doctor = await assertDoctorGraph(projection, "Offline import");
  const summary = await evaluate(
    `(() => { const facts = Array.from(document.querySelectorAll('.workspace-doctor .doctor-summary-facts > div')).map((item) => ({ label: item.querySelector('dt')?.textContent?.trim() ?? null, value: item.querySelector('dd')?.textContent?.trim() ?? null })); return { facts, actions: Array.from(document.querySelectorAll('.context-bar .context-actions button')).map((button) => button.textContent.trim()), recommendationsCount: document.querySelector('.workspace-nav button[data-step="3"]')?.getAttribute('data-count') ?? null }; })()`,
  );
  const total = summary.facts.find((item) => item.label === "Total time");
  assert(
    total?.value && total.value !== "0" && total.value !== "0 ns",
    "Doctor summary did not render the backend Profile total time.",
  );
  assert(
    summary.actions.includes("Compare before / after") && /^\d+$/.test(summary.recommendationsCount ?? ""),
    "Doctor context lost its Compare entry or the Recommendations count badge.",
  );
  receipt.offline_doctor.summary_facts = summary.facts.map((item) => item.label);
  receipt.assertions.push("offline_import_doctor_summary_graph_plan_node_table");
  if (complexProfile) await retainedComplexProfileCoverage(projection);
  await capture("offline-doctor-1440x1024");
  for (const workspace of ["Recommendations", "Compare", "Doctor"]) {
    await clickButton(workspace);
    await waitFor(
      () => has(`.workspace-${workspace.toLowerCase()}`),
      `${workspace} workspace`,
    );
    if (workspace !== "Doctor") await capture(`offline-${workspace.toLowerCase()}-1440x1024`);
  }
  receipt.assertions.push("offline_import_doctor_recommendations_compare");
}
async function retainedComplexProfileCoverage(projection) {
  const graph = projection.graph;
  receipt.complex_profile.backend_graph = graph;
  // 27 PlanChild plus 8 FragmentSink evidence edges collapse into 27 directed
  // node pairs; the official ExplainAnalyzer summary covers all 28 nodes.
  assert(
    graph?.node_count === 28 &&
      graph.edge_count === 35 &&
      graph.data_flow_pair_count === 27 &&
      graph.summary_state === "available" &&
      graph.summary_node_count === 28,
    "Retained complex Profile lost its 28-node/35-evidence/27-pair structure or its execution summary.",
  );
  assert(
    graph.hottest?.node_id === "0" &&
      graph.hottest.basis_points === 1728 &&
      graph.hottest.hotspot === "second_most_consuming",
    "Retained complex Profile hottest node drifted from the ExplainAnalyzer golden.",
  );
  const root = ".workspace-doctor .execution-graph";
  const firstRow = (await doctorGraphDom(root)).first_table_row ?? "";
  assert(
    (await doctorGraphDom(root)).first_table_row_plan === "0" && firstRow.includes("17.28%"),
    "Plan-node table did not rank the hottest node first with its backend share.",
  );
  const viewportState = () =>
    evaluate(
      `(() => {    const root = document.querySelector(${JSON.stringify(root)});    const viewport = root?.querySelector('.react-flow__viewport');    return { transform: viewport?.getAttribute('style') ?? null };  })()`,
    );
  const beforeLocate = await viewportState();
  await clickButton("Fit all");
  await waitFor(
    async () => (await viewportState()).transform !== beforeLocate.transform,
    "Fit all moves the actual viewport",
  );
  const fitted = await viewportState();
  // The header's node/hotspot count locates the hottest node (design board 02).
  await clickSelector(`${root} .graph-counts`);
  await waitFor(
    () =>
      evaluate(
        `(() => {    const root = document.querySelector(${JSON.stringify(root)});    const node = root?.querySelector('.plan-card.selected');    const canvas = root?.querySelector('.execution-graph-canvas');    if (!node || node.dataset.nodeId !== '0' || canvas?.dataset.layoutState !== 'ready' || !root.querySelector('.node-inspector')) return false;    const n = node.getBoundingClientRect(), c = canvas.getBoundingClientRect();    return n.width > 0 && n.height > 0 && n.left >= Math.max(c.left, 0) && n.right <= Math.min(c.right, innerWidth)      && n.top >= Math.max(c.top, 0) && n.bottom <= Math.min(c.bottom, innerHeight);  })()`,
      ),
    "Hottest node selected and centered in the viewport",
  );
  assert(
    (await viewportState()).transform !== fitted.transform,
    "Hottest node locate did not move the viewport.",
  );
  const inspector = await textOf(`${root} .node-inspector`);
  assert(
    inspector.includes("17.28%") && inspector.includes("Output rows"),
    "Node inspector lost the backend share or output rows.",
  );
  assert(
    (await evaluate(`document.querySelector('.workspace-doctor .doctor-details [role="tab"][aria-selected="true"]')?.textContent?.startsWith('Operators') === true`)),
    "Hottest node navigation unexpectedly opened the original Profile.",
  );
  receipt.complex_profile.hottest_located_in_viewport = true;
  receipt.assertions.push("retained_complex_hottest_node_located_with_inspector");
  await capture("retained-complex-hottest-1440x1024");
  await clickReactFlowPaneBackground(`${root} .react-flow__pane`);
  await waitFor(
    async () => (await doctorGraphDom(root)).selected_node_ids.length === 0,
    "pane click clears node selection",
  );
  receipt.assertions.push(
    "retained_complex_profile_28_nodes_35_evidence_edges_27_pairs_summary_hottest_visible",
  );
}
async function auditFilterDrawerState() {
  return evaluate(
    `(() => {    const bar = document.querySelector(".audit-filter-bar");    const more = document.getElementById("audit-more-filters");    const toggle = bar?.querySelector("button.audit-more-filters") ?? null;    const controls = bar ? [      bar.querySelector('select[aria-label="Audit window"]'),      bar.querySelector('button[type="submit"]'),    ] : [];    const visible = (element) => {      if (!(element instanceof HTMLElement)) return false;      const style = getComputedStyle(element);      const rect = element.getBoundingClientRect();      return style.display !== "none" && style.visibility !== "hidden" && Number(style.opacity) !== 0 && rect.width > 0 && rect.height > 0;    };    return {      drawer_present: bar !== null,      disclosure: more !== null && toggle !== null,      open: more ? !more.hidden : null,      summary_visible: visible(toggle),      controls_visible: controls.length === 2 && controls.every(visible),    };  })()`,
  );
}
async function prepareAuditFixtureFilterControls(requireOverviewDisclosure) {
  let state = await auditFilterDrawerState();
  if (requireOverviewDisclosure) {
    assert(
      state.drawer_present && state.disclosure,
      "Fixed Audit fixture must use the Overview filter bar with More filters.",
    );
  }
  if (state.disclosure && !state.open) {
    assert(
      state.summary_visible,
      "Fixed Audit fixture More filters toggle was not visible.",
    );
    await clickElement(
      'document.querySelector(".audit-filter-bar button.audit-more-filters")',
      "Fixed Audit fixture More filters toggle was unavailable.",
    );
  }
  await waitFor(async () => {
    state = await auditFilterDrawerState();
    return (
      state.drawer_present &&
      (!requireOverviewDisclosure || state.disclosure) &&
      (!state.disclosure || state.open === true) &&
      state.controls_visible
    );
  }, "fixed Audit fixture visible filter controls");
  return state;
}
async function openAuditFixtureFilterDrawer() {
  return prepareAuditFixtureFilterControls(true);
}
async function applyAuditFixtureWindow(
  route,
  description,
  requireOverviewDisclosure,
  responseSelector,
) {
  if (!auditFixtureWindow) return null;
  const expectedSeconds = auditWindowSecondsByPreset[auditFixtureWindow];
  const routesBefore = (await routeTelemetry(route)).length;
  const projectionsBefore = (await projectionTelemetry(route)).length;
  await (requireOverviewDisclosure
    ? openAuditFixtureFilterDrawer()
    : prepareAuditFixtureFilterControls(false));
  await setInputValue('select[aria-label="Audit window"]', auditFixtureWindow);
  const selectedWindow = await evaluate(
    "document.querySelector('select[aria-label=\"Audit window\"]')?.value ?? null",
  );
  assert(
    selectedWindow === auditFixtureWindow,
    "Fixed Audit fixture window did not commit in the real control.",
  );
  await clickElement(
    `document.querySelector(".audit-filter-bar button[type='submit']")`,
    "Fixed Audit fixture Apply control was unavailable.",
  );
  await waitFor(async () => {
    const routes = await routeTelemetry(route);
    return (
      routes.length === routesBefore + 1 &&
      routes.at(-1)?.method === "POST" &&
      routes.at(-1)?.status === 200 &&
      (await has(responseSelector))
    );
  }, `${description} Apply`);
  const applied = await projectionAfterRoute(
    route,
    projectionsBefore,
    description,
  );
  assert(
    applied.audit_scope?.window_seconds === expectedSeconds,
    "Fixed Audit fixture Apply did not publish the selected scope.",
  );
  return applied;
}
async function applyAuditFixtureWindowAfterOverview() {
  const applied = await applyAuditFixtureWindow(
    auditOverviewRoute,
    "Fixed Audit fixture Overview",
    true,
    ".audit-kpi-grid",
  );
  if (!applied) return null;
  receipt.audit_fixture_window = {
    requested: auditFixtureWindow,
    expected_seconds: auditWindowSecondsByPreset[auditFixtureWindow],
    applied_scope: applied.audit_scope,
  };
  return applied.audit_scope;
}
async function auditWorkloadCoverage() {
  await evaluate(auditReleaseObserverSource());
  const overviewRequestsBefore = (await routeTelemetry(auditOverviewRoute))
    .length;
  await openQueryMode("Audit history");
  await waitFor(
    () => has(".workspace-queries .audit-workload-overview"),
    "Audit history query mode",
  );
  const auditWindowOptions = await evaluate(
    `Array.from(document.querySelectorAll('select[aria-label="Audit window"] option')).map((option) => option.value)`,
  );
  assert(
    JSON.stringify(auditWindowOptions) ===
      JSON.stringify(expectedAuditWindowPresets),
    `Audit window presets drifted: ${JSON.stringify(auditWindowOptions)}`,
  );
  receipt.audit_window_presets = auditWindowOptions;
  receipt.assertions.push("audit_six_window_presets");
  await waitFor(async () => {
    const routes = await routeTelemetry(auditOverviewRoute);
    return (
      routes.length > overviewRequestsBefore &&
      routes.at(-1)?.method === "POST" &&
      routes.at(-1)?.status === 200 &&
      (await has(".audit-kpi-grid"))
    );
  }, "Audit Overview response");
  const overviewProjection = (await projectionTelemetry(auditOverviewRoute)).at(-1);
  const overviewTop = overviewProjection?.audit_top;
  const overviewCoverage = overviewProjection?.audit_coverage;
  assert(overviewProjection?.audit_primary_state === "families", "Default Audit Overview must land on repeated SQL (design board 01).");
  assert(overviewTop && typeof overviewTop.full_window_count === "string" &&
    Number.isInteger(overviewTop.requested_limit) && Number.isInteger(overviewTop.returned_count) &&
    typeof overviewCoverage?.retained_event_count === "string" &&
    overviewTop.full_window_count === overviewCoverage.retained_family_count &&
    typeof overviewCoverage.missing_digest_count === "string",
    "Audit Overview response lost its exact full-window count and coverage facts.");
  receipt.audit_overview = await evaluate(
    `(() => { const root = document.querySelector('.workspace-queries .audit-workload-overview'); const foot = root?.querySelector('.audit-board-main-foot')?.textContent ?? ''; return {
      summary_cards: root?.querySelectorAll('.audit-kpi-grid .audit-kpi').length ?? 0,
      query_count: Array.from(root?.querySelectorAll('.audit-kpi') ?? []).find(card => card.querySelector('.audit-kpi-label')?.textContent === 'Query count')?.querySelector('.audit-kpi-value')?.textContent?.replaceAll(',', '') ?? null,
      has_time_range: (root?.querySelector('select[aria-label="Audit window"]')?.getAttribute('title') ?? '').includes(' → '),
      has_returned_count: foot.includes(${JSON.stringify(`${overviewCoverage.retained_family_count} query families`)}),
    }; })()`,
  );
  assert(receipt.audit_overview.summary_cards === 5 &&
    receipt.audit_overview.query_count === overviewCoverage.retained_event_count &&
    receipt.audit_overview.has_time_range && receipt.audit_overview.has_returned_count,
    "Audit Overview did not render its DTO-backed summary counts and selected time range.");
  receipt.audit_overview.backend_top = overviewTop;
  receipt.audit_overview.backend_coverage = overviewCoverage;
  receipt.assertions.push("audit_overview_http200_full_window_cards_coverage");
  await captureAuditResourceProbe({ evaluate, receipt, runRoot });
  await showAuditIndividualExecutions("Audit individual executions");
  if (project === "connected-readonly-docker") await applyAuditReleaseCohort("workload_user");
  await captureAuditReleaseOverview({ evaluate, assert, receipt });
  // The legacy filter regression intentionally starts from an unfiltered scope.
  // Keep its contract separate from the already captured immutable A05 cohort.
  if (project === "connected-readonly-docker") await applyAuditReleaseCohort("");
  await applyAuditFixtureWindowAfterOverview();
  await waitFor(
    () => has('.workspace-queries table[aria-label="Audited query rankings"]'),
    "Audit history occurrence rankings",
  );
  const sqlRoutesBeforeSelection = (await routeTelemetry("/audit/sql")).length;
  const executionRoutesBeforeSelection = (
    await routeTelemetry("/runtime-profile")
  ).length;
  const explainRoutesBeforeSelection = (await routeTelemetry("/explain"))
    .length;
  await clickElement(
    `Array.from(document.querySelectorAll('button[aria-label^="Select audited query "]')).find((item) => !item.disabled)`,
    "No audited execution was available for explicit selection.",
  );
  await waitFor(() => has(".audit-selection:not(.audit-selection-empty)"), "selected audited execution");
  // With the detail panel open at 1440 wide, the 11-column table (with SQL) must not scroll sideways.
  receipt.audit_occurrence_layout = await evaluate(
    `(() => { const table = document.querySelector('.workspace-queries table[aria-label="Audited query rankings"]'); const wrap = table?.closest('.audit-table-wrap'); const cells = Array.from(table?.querySelectorAll('tbody td.audit-cell-sql') ?? []); return {
      viewport_width: window.innerWidth, page: [document.documentElement.clientWidth, document.documentElement.scrollWidth],
      wrap: wrap ? [wrap.clientWidth, wrap.scrollWidth] : null, sql_column_width: table?.querySelectorAll('thead th')[1]?.offsetWidth ?? 0,
      sql_cells: cells.length, rows: table?.querySelectorAll('tbody tr').length ?? 0,
      empty_sql_cells: cells.filter((cell) => cell.textContent.trim() === '').length,
    }; })()`,
  );
  assert(
    receipt.audit_occurrence_layout.viewport_width === 1440 &&
      receipt.audit_occurrence_layout.wrap !== null &&
      receipt.audit_occurrence_layout.wrap[1] <= receipt.audit_occurrence_layout.wrap[0] &&
      receipt.audit_occurrence_layout.page[1] <= receipt.audit_occurrence_layout.page[0] &&
      receipt.audit_occurrence_layout.sql_column_width >= 120 &&
      receipt.audit_occurrence_layout.sql_cells === receipt.audit_occurrence_layout.rows &&
      receipt.audit_occurrence_layout.empty_sql_cells === 0,
    "Audit individual executions scroll sideways with the detail panel open, or a SQL cell is blank.",
  );
  receipt.assertions.push("audit_occurrence_sql_column_no_horizontal_scroll_with_detail_panel");
  assert(
    (await routeTelemetry("/audit/sql")).length === sqlRoutesBeforeSelection,
    "Selecting an Audit row read SQL before an explicit request.",
  );
  await clickButton("View SQL");
  await waitFor(async () => {
    const routes = await routeTelemetry("/audit/sql");
    return (
      routes.length === sqlRoutesBeforeSelection + 1 &&
      routes.at(-1)?.method === "POST" &&
      routes.at(-1)?.status === 200 &&
      (await has(".audit-sql pre"))
    );
  }, "explicit Audit SQL read");
  assert(
    (await routeTelemetry("/runtime-profile")).length ===
      executionRoutesBeforeSelection,
    "Audit SQL view executed a Runtime Profile request.",
  );
  assert(
    (await routeTelemetry("/explain")).length === explainRoutesBeforeSelection,
    "Audit SQL view executed an Explain request.",
  );
  receipt.audit_sql = await evaluate(
    `({ visible: Boolean(document.querySelector('.audit-sql pre')), character_count: document.querySelector('.audit-sql pre')?.textContent?.length ?? 0 })`,
  );
  assert(
    receipt.audit_sql.character_count > 0,
    "Explicit Audit SQL read returned no visible SQL.",
  );
  receipt.assertions.push("audit_queries_explicit_row_sql_read_no_execute");
  // A window whose retained events all lack a native digest cannot form a
  // family; the product replaces the family entry with enable_sql_digest
  // guidance instead of an empty grouping panel.
  // The guidance is drawn only on the families view, and the steps above leave
  // the individual-executions view open, so digest absence is decided from the
  // backend's latest overview coverage rather than from what this view shows.
  const familiesOffered = await evaluate(`Array.from(document.querySelectorAll(".audit-view-switch button")).some((button) => button.textContent.trim() === "Repeated SQL")`);
  const latestCoverage = (await projectionTelemetry(auditOverviewRoute)).at(-1)?.audit_coverage ?? null;
  const digestFree = /^[1-9][0-9]*$/.test(latestCoverage?.retained_event_count ?? "") &&
    latestCoverage.missing_digest_count === latestCoverage.retained_event_count;
  if (digestFree || (await has(".workspace-queries .audit-digest-guidance"))) {
    assert(!familiesOffered, "A digest-free Audit window still offered repeated-SQL grouping.");
    receipt.audit_families = { row_count: 0, exact_native_digest_drill_count: 0, drill_state: "not_attempted_digest_unavailable_guidance_visible" };
  } else {
    assert(familiesOffered, "Repeated-SQL grouping was hidden although the Audit window has native digests.");
    const familyRequestsBefore = (await routeTelemetry(auditOverviewRoute)).length;
    await clickButton("Repeated SQL");
    await waitFor(async () => {
      const routes = await routeTelemetry(auditOverviewRoute);
      return (
        routes.length === familyRequestsBefore + 1 &&
        routes.at(-1)?.method === "POST" &&
        routes.at(-1)?.status === 200 &&
        (await has('table[aria-label="Audited query families"]'))
      );
    }, "Audit family response");
    receipt.audit_families = await evaluate(
      `(() => ({    row_count: document.querySelectorAll('table[aria-label="Audited query families"] tbody tr').length,    exact_native_digest_drill_count: Array.from(document.querySelectorAll('table[aria-label="Audited query families"] button[aria-label^="Select query family "]')).filter((button) => !button.disabled).length,  }))()`,
    );
  }
  if (receipt.audit_families.drill_state) {
    // Already decided above: no digest means no family to drill into.
  } else if (receipt.audit_families.row_count === 0) {
    receipt.audit_families.drill_state = "not_attempted_no_family_rows";
  } else if (receipt.audit_families.exact_native_digest_drill_count === 0) {
    receipt.audit_families.drill_state = "not_attempted_no_exact_native_digest";
  } else {
    const familyDetailsBefore = (
      await projectionTelemetry("/audit/family-detail")
    ).length;
    const detailRequestsBefore = (await routeTelemetry("/audit/family-detail"))
      .length;
    const sqlBeforeDetail = (await routeTelemetry("/audit/sql")).length;
    const profilesBeforeDetail = (await routeTelemetry("/profiles/fetch"))
      .length;
    const memberReadsBefore = (await routeTelemetry("/audit/occurrences")).length;
    await openFirstAuditFamilyDistribution(
      `Array.from(document.querySelectorAll('table[aria-label="Audited query families"] button[aria-label^="Select query family "]')).find((button) => !button.disabled)`,
      "Exact native-digest family drill button disappeared.",
    );
    assert(
      (await routeTelemetry("/audit/occurrences")).length === memberReadsBefore + 1 &&
        (await routeTelemetry("/audit/sql")).length === sqlBeforeDetail &&
        (await routeTelemetry("/profiles/fetch")).length === profilesBeforeDetail,
      "Selecting a family read anything beyond its one member ranking.",
    );
    receipt.assertions.push("audit_family_selection_one_member_read_no_sql_no_profile");
    await waitFor(async () => {
      const routes = await routeTelemetry("/audit/family-detail");
      return (
        routes.length === detailRequestsBefore + 1 &&
        routes.at(-1)?.method === "POST" &&
        routes.at(-1)?.status === 200 &&
        (await has(".audit-family-detail"))
      );
    }, "exact native-digest family detail response");
    const detailProjection = await projectionAfterRoute(
      "/audit/family-detail",
      familyDetailsBefore,
      "Family detail",
    );
    const detail = detailProjection.family_detail;
    assert(
      detail?.membership_state === "exact_native_digest" &&
        detail.distribution_state === "exact" &&
        detail.bucket_count === 8,
      "Family detail did not return its exact eight-bucket contract.",
    );
    assert(
      (await routeTelemetry("/audit/sql")).length === sqlBeforeDetail &&
        (await routeTelemetry("/profiles/fetch")).length ===
          profilesBeforeDetail,
      "Opening family detail implicitly fetched SQL or Profile.",
    );
    assert(
      (await evaluate(
        `document.querySelectorAll('.audit-family-buckets > div').length`,
      )) === detail.bucket_count,
      "Family detail rendered a different bucket count than its DTO.",
    );
    const bucketGeometry = await evaluate(
      `Array.from(document.querySelectorAll('.audit-family-buckets meter')).map(meter => ({ width: meter.getBoundingClientRect().width, right: meter.getBoundingClientRect().right, parentRight: meter.closest('dd').getBoundingClientRect().right, height: meter.getBoundingClientRect().height }))`,
    );
    assert(
      bucketGeometry.length === 8 &&
        bucketGeometry.every(
          (bar) =>
            bar.width >= 120 &&
            bar.height >= 10 &&
            bar.right <= bar.parentRight + 1,
        ),
      "Distribution bars collapsed or overflowed their card.",
    );
    receipt.audit_families.bucket_geometry = bucketGeometry;
    receipt.assertions.push("audit_family_distribution_bars_readable_width");
    receipt.audit_families.detail = detail;
    receipt.audit_families.drill_state = "exact_native_digest_detail_loaded";
    receipt.assertions.push(
      "audit_family_detail_eight_buckets_no_implicit_evidence",
    );
    await capture("audit-family-detail-1440x1024");
    const familyFactsBeforeEvidence = await textOf(".audit-family-detail");
    if (detail.representative_states[1] === "selected") {
      const runtimeBeforeSql = (await routeTelemetry("/runtime-profile"))
        .length;
      await clickButton("View SQL for slowest representative");
      await waitFor(async () => {
        const routes = await routeTelemetry("/audit/sql");
        return (
          routes.length === sqlBeforeDetail + 1 &&
          ((routes.at(-1)?.status === 200 && (await has(".audit-sql pre"))) ||
            (project === "connected-readonly-docker" && routes.length === sqlBeforeDetail + 1 && routes.at(-1)?.status >= 400 && (await has(".audit-sql-error"))))
        );
      }, "explicit slowest representative SQL read");
      assert(
        (await routeTelemetry("/runtime-profile")).length ===
          runtimeBeforeSql &&
          (await routeTelemetry("/profiles/fetch")).length ===
            profilesBeforeDetail,
        "Representative SQL view unexpectedly executed or fetched a Profile.",
      );
      receipt.assertions.push("audit_family_slowest_sql_explicit_read_only");
      receipt.audit_release.representative_sql = (await has(".audit-sql-error")) ? "real_typed_unavailable_facts_retained" : "real_success_unavailability_not_exercised";
      assert((await textOf(".audit-family-detail")) === familyFactsBeforeEvidence, "Representative SQL action lost Audit family facts.");
    }
    if (detail.representative_states[2] === "selected") {
      const profileProjectionsBefore = (
        await projectionTelemetry("/profiles/fetch")
      ).length;
      const sqlBeforeProfile = (await routeTelemetry("/audit/sql")).length;
      const runtimeBeforeProfile = (await routeTelemetry("/runtime-profile"))
        .length;
      await clickButton("Fetch Profile for evidence representative");
      await waitFor(async () => {
        const routes = await routeTelemetry("/profiles/fetch");
        return (
          routes.length === profilesBeforeDetail + 1 &&
          ((routes.at(-1)?.status === 200 && (await has(".audit-profile-receipt"))) ||
            (project === "connected-readonly-docker" && routes.length === profilesBeforeDetail + 1 && routes.at(-1)?.status >= 400 && (await has(".audit-error"))))
        );
      }, "explicit evidence representative retained Profile");
      assert((await textOf(".audit-family-detail")) === familyFactsBeforeEvidence, "Representative Profile action lost Audit family facts.");
      const profileUnavailable = await has(".audit-error");
      receipt.audit_release.representative_profile = profileUnavailable ? "real_typed_unavailable_facts_retained" : "real_success_unavailability_not_exercised";
      assert((await routeTelemetry("/audit/sql")).length === sqlBeforeProfile && (await routeTelemetry("/runtime-profile")).length === runtimeBeforeProfile, "Representative Profile failure triggered SQL or execution fallback.");
      if (!profileUnavailable) {
      const fetched = await projectionAfterRoute(
        "/profiles/fetch",
        profileProjectionsBefore,
        "Family representative Profile",
      );
      assert(
        fetched.profile_fetch_state === "fetched",
        "Representative Profile did not produce a fetched report.",
      );
      assert(
        (await routeTelemetry("/audit/sql")).length === sqlBeforeProfile &&
          (await routeTelemetry("/runtime-profile")).length ===
            runtimeBeforeProfile,
        "Fetching an existing family Profile executed or read SQL implicitly.",
      );
      assert(
        await evaluate(
          `Array.from(document.querySelectorAll('.audit-selection-actions button')).some(button => button.textContent.trim() === 'Open diagnosis' && !button.disabled)`,
        ),
        "Fetched representative report was not actionable in Doctor.",
      );
      receipt.assertions.push(
        "audit_family_evidence_profile_explicit_fetch_no_execution",
      );
      await capture("audit-family-profile-fetched-1440x1024");
      }
    } else {
      receipt.audit_release.representative_profile = "no_evidence_representative_unavailability_not_exercised";
      assert(
        project !== "connected-audit-docker",
        "Fixed Audit fixture has no retained representative Profile; acceptance remains unexercised.",
      );
    }
    const memberProjectionsBefore = (
      await projectionTelemetry(auditOverviewRoute)
    ).length;
    const memberRequestsBefore = (await routeTelemetry(auditOverviewRoute))
      .length;
    await clickButton("Browse member rankings");
    await waitFor(async () => {
      const routes = await routeTelemetry(auditOverviewRoute);
      return (
        routes.length === memberRequestsBefore + 1 &&
        routes.at(-1)?.status === 200 &&
        (await has(".audit-family-summary"))
      );
    }, "explicit family member rankings");
    const members = await projectionAfterRoute(
    auditOverviewRoute,
      memberProjectionsBefore,
      "Family member rankings",
    );
    assert(
      members.audit_scope?.has_digest_filter &&
        members.audit_scope.has_database_filter &&
        members.audit_scope.window_seconds ===
          detailProjection.audit_scope.window_seconds,
      "Member rankings lost the bounded exact-family scope.",
    );
    assert(
      members.audit_generation > detailProjection.audit_generation,
      "Member rankings did not publish a new discovery generation.",
    );
    receipt.assertions.push(
      "audit_family_member_rankings_bound_scope_new_generation",
    );
    await capture("audit-family-member-rankings-1440x1024");
    assert(
      !(await has(".audit-sql pre")) && !(await has(".audit-profile-receipt")),
      "Changing discovery to member rankings retained representative evidence.",
    );
  }
  receipt.assertions.push(
    `audit_family_native_digest:${receipt.audit_families.drill_state}`,
  );
  await auditFilterCoverage();
  if (project === "connected-readonly-docker" && process.env.E2E_MISSING_PROFILE_DIGEST) {
    await captureMissingProfileFamily({ evaluate, check: assert, clickButton, setInputValue, clickElement, waitFor, routeTelemetry, capture, receipt,
      digest: process.env.E2E_MISSING_PROFILE_DIGEST,
      queryHash: createHash("sha256").update(requiredEnv("E2E_MISSING_PROFILE_QUERY_ID").toLowerCase()).digest("hex") });
  }
  if (project === "connected-audit-docker") await auditFamilyDoctorCoverage();
  await captureAuditReleaseRequests({ evaluate, assert, receipt });
}
async function applyAuditReleaseCohort(user) {
  await openAuditFixtureFilterDrawer();
  assert(await evaluate(`document.querySelector('input[aria-label="Exact user filter"]')?.disabled === false`), "A05 requires the real exact-user filter capability.");
  // D2: the official fixture table has TINYINT isQuery and stmt, so the explicit
  // statement filter is enabled and on by default; the oracle restates it.
  assert(await evaluate(`(() => { const box = document.querySelector('input[aria-describedby="audit-statement-filter-note"]'); return box?.disabled === false && box.checked === true; })()`), "A05 requires the default-on statement filter.");
  await setInputValue('select[aria-label="Audit window"]', "1h");
  await setInputValue('input[aria-label="Exact user filter"]', user);
  const before = (await routeTelemetry(auditOverviewRoute)).length;
  await clickElement(`document.querySelector(".audit-filter-bar button[type='submit']")`, "A05 cohort Apply unavailable.");
  await waitFor(async () => {
    const routes = await routeTelemetry(auditOverviewRoute);
    return routes.length === before + 1 && routes.at(-1)?.status === 200 &&
      await evaluate(`document.querySelector('.workspace-queries .audit-workload')?.getAttribute('aria-busy') === 'false' && !document.querySelector('.audit-window-evidence')?.textContent.includes('Filters changed. Apply them to update the results.')`);
  }, "A05 explicit one-hour exact-user scope committed");
}
/** Select a family (one member read), then open its members and the distribution detail. */
async function openFirstAuditFamilyDistribution(selectFamilyExpression, missingMessage) {
  await clickElement(selectFamilyExpression, missingMessage);
  await waitFor(
    async () => (await has(".audit-family-aside .audit-family-members-more")) || (await has(".audit-family-aside-error")),
    "selected family member ranking",
  );
  assert(!(await has(".audit-family-aside-error")), "Selected family member ranking failed.");
  await clickElement('document.querySelector(".audit-family-aside .audit-family-members-more")', "Family member-table entry was unavailable.");
  await waitFor(() => has('.audit-board-main table[aria-label="Audited query rankings"]'), "family members table");
  await clickButton("Distribution and representatives");
}
async function showAuditIndividualExecutions(description) {
  if (await has('.workspace-queries table[aria-label="Audited query rankings"]')) return;
  const before = (await routeTelemetry(auditOverviewRoute)).length;
  await clickButton("Individual executions");
  await waitFor(async () => {
    const routes = await routeTelemetry(auditOverviewRoute);
    return routes.length === before + 1 && routes.at(-1)?.status === 200 &&
      (await has('.workspace-queries table[aria-label="Audited query rankings"]'));
  }, description);
}
async function auditFamilyDoctorCoverage() {
  const familiesBefore = (await routeTelemetry(auditOverviewRoute)).length;
  await clickButton("Repeated SQL");
  await waitFor(
    async () =>
      (await routeTelemetry(auditOverviewRoute)).length === familiesBefore + 1 &&
      (await has('table[aria-label="Audited query families"]')),
    "family discovery for Doctor handoff",
  );
  const familyRows = await evaluate(
    "document.querySelectorAll('table[aria-label=\"Audited query families\"] tbody tr').length",
  );
  if (familyRows === 0) {
    const fallbackFamiliesBefore = (await routeTelemetry(auditOverviewRoute))
      .length;
    await setInputValue('select[aria-label="Audit window"]', "30d");
    await clickButton("Apply filters");
    await waitFor(async () => {
      const routes = await routeTelemetry(auditOverviewRoute);
      return (
        routes.length === fallbackFamiliesBefore + 1 &&
        routes.at(-1)?.method === "POST" &&
        routes.at(-1)?.status === 200 &&
        (await has('table[aria-label="Audited query families"]'))
      );
    }, "bounded 30d family discovery for Doctor handoff");
  }
  await openFirstAuditFamilyDistribution(
    `Array.from(document.querySelectorAll('table[aria-label="Audited query families"] button[aria-label^="Select query family "]')).find(button => !button.disabled)`,
    "No exact family available for Doctor handoff.",
  );
  await waitFor(
    () => has(".audit-family-detail"),
    "family detail for Doctor handoff",
  );
  const profilesBefore = (await projectionTelemetry("/profiles/fetch")).length;
  await clickButton("Fetch Profile for evidence representative");
  await waitFor(
    () => has(".audit-profile-receipt"),
    "retained Profile for Doctor handoff",
  );
  const projection = await projectionAfterRoute(
    "/profiles/fetch",
    profilesBefore,
    "Family Doctor Profile",
  );
  assert(
    projection.profile_fetch_state === "fetched" &&
      projection.graph?.node_count > 0,
    "Family Profile lacks a measured readable projection.",
  );
  const fetchBeforeOpen = (await routeTelemetry("/profiles/fetch")).length;
  const runtimeBeforeOpen = (await routeTelemetry("/runtime-profile")).length;
  await clickElement(
    `Array.from(document.querySelectorAll('.audit-selection-actions button')).find(button => button.textContent.trim() === 'Open diagnosis' && !button.disabled)`,
    "Selected Audit Profile diagnosis action was unavailable.",
  );
  await waitFor(
    () => has(".workspace-doctor"),
    "family Profile Doctor workspace",
  );
  receipt.audit_family_doctor = await assertDoctorGraph(
    projection,
    "Family retained Profile",
  );
  assert(
    (await routeTelemetry("/profiles/fetch")).length === fetchBeforeOpen &&
      (await routeTelemetry("/runtime-profile")).length === runtimeBeforeOpen,
    "Opening Doctor fetched or executed another query.",
  );
  receipt.assertions.push(
    "audit_family_retained_profile_doctor_dto_graph_no_rerun",
  );
  await capture("audit-family-doctor-1440x1024");
  await officialProfileCoverage();
}
async function officialProfileCoverage() {
  const result = {
    transport: officialProfileTransport,
    assertions: [],
    request_counts: { before: 0, after: 0 },
    responses: [],
    mock_restored:
      officialProfileTransport === "real_loopback_api" ? null : false,
  };
  receipt.official_profile = result;
  let profileIdentity = await evaluate(
    "window.__doctorE2eLastProfileFetchIdentity",
  );
  assert(
    profileIdentity?.connection_id_sha256 &&
      profileIdentity?.query_id_sha256 &&
      profileIdentity?.identity?.kind &&
      (profileIdentity.identity.kind !== "discovery" ||
        Number.isInteger(profileIdentity.discovery_generation)),
    "Official reading has no current tagged fetched Profile identity evidence.",
  );
  const nodeCandidate = await evaluate(
    "(window.__doctorE2eOfficialNodeCandidates ?? []).find((node) => node && typeof node.node_id === 'string') ?? null",
  );
  assert(
    nodeCandidate,
    "Official node scope has no current DTO plan-node evidence; node acceptance remains unexercised.",
  );
  let mockInstalled = false;
  try {
    if (officialProfileTransport === "controlled_mock_over_real_session") {
      await installOfficialMock();
      mockInstalled = true;
      await configureOfficialMockQueue([]);
    }
    await waitFor(
      () =>
        has(
          '.workspace-doctor .execution-graph-canvas[data-layout-state="ready"]',
        ),
      "Doctor execution graph layout before official reading",
    );
    // The preceding Doctor checks may have left a plan node selected. Clear
    // the real SelectionProvider through the visible React Flow pane so the
    // no-selection scope assertion is about current DOM state, not a hidden
    // prior card.
    const flowPaneSelector =
      ".workspace-doctor .execution-graph .react-flow__pane";
    await waitFor(
      () => has(flowPaneSelector),
      "Doctor React Flow pane before official no-selection check",
    );
    await clickReactFlowPaneBackground(flowPaneSelector);
    await waitFor(
      () =>
        has(
          ".workspace-doctor .execution-graph .plan-card.selected",
        ).then((selected) => !selected),
      "selection cleared by actual React Flow pane click",
    );
    result.no_selection_clear = "react_flow_pane_background";
    result.request_counts.before = (await officialRequestLog()).length;
    await clickButton("Official reading");
    await waitFor(
      () =>
        has('#official-profile-dialog[aria-label="Official profile reading"]'),
      "official reading dialog",
    );
    assert(
      (await officialRequestLog()).length === result.request_counts.before,
      "Opening the official panel issued a hidden read.",
    );
    result.assertions.push("open_zero_official_requests");
    const noSelection = await evaluate(
      `(() => {      const selected = document.querySelector('.workspace-doctor .execution-graph .plan-card.selected');      const nodeScope = document.querySelector('.official-profile-scope button:nth-child(2)');      return { selected: Boolean(selected), node_disabled: nodeScope?.disabled ?? null };    })()`,
    );
    assert(
      !noSelection.selected && noSelection.node_disabled === true,
      "Official node scope was not disabled when no plan node was selected.",
    );
    result.no_selected_node_button_disabled = true;
    result.assertions.push("no_selected_node_scope_disabled");
    if (officialProfileTransport === "controlled_mock_over_real_session") {
      await configureOfficialMockQueue([{ kind: "hold_native" }]);
      const beforeAbort = (await officialRequestLog()).length;
      await clickButton("Read official analysis");
      await waitFor(
        async () =>
          (await officialRequestLog()).length === beforeAbort + 1 &&
          (await has('.official-profile-action[aria-busy="true"]')),
        "official loading state and held request",
      );
      await clickSelector(".official-profile-return");
      await waitFor(
        () => has("#official-profile-dialog").then((present) => !present),
        "official reading cancel close",
      );
      await waitFor(
        async () => (await officialRequestLog()).at(-1)?.aborted === true,
        "official AbortSignal after cancel",
      );
      await releaseOfficialMock();
      await wait(250);
      await clickButton("Official reading");
      await waitFor(
        () => has("#official-profile-dialog"),
        "official reading reopen after late reply",
      );
      assert(
        !(await has(".official-profile-success")),
        "Late official response repopulated a closed reading panel.",
      );
      result.abort_late_reply = {
        request_count: (await officialRequestLog()).length,
        aborted: true,
        stale_success_dropped: true,
      };
      result.assertions.push("close_abort_late_reply_dropped");
      await configureOfficialMockQueue([]);
    }
    const wholeBefore = (await officialRequestLog()).length;
    await clickButton("Read official analysis");
    await waitFor(async () => {
      const requests = await officialRequestLog();
      return (
        requests.length === wholeBefore + 1 && requests.at(-1)?.status === 200
      );
    }, "whole-query official response");
    await waitFor(
      async () =>
        (await officialResponseLog()).length >= 1 &&
        (await has(".official-profile-success")),
      "whole-query official text",
    );
    const wholeRequest = (await officialRequestLog()).at(-1);
    const wholeResponse = (await officialResponseLog()).at(-1);
    assertOfficialRequest(wholeRequest, [], profileIdentity);
    assertOfficialResponse(wholeResponse, [], profileIdentity);
    assertOfficialIdentityEcho(wholeRequest, wholeResponse);
    await recordOfficialSuccess(result, "whole_query", wholeResponse);
    const displayFocus = await evaluate(
      `(() => { const display = document.querySelector('.official-profile-display-text'); display?.focus(); return document.activeElement === display; })()`,
    );
    assert(displayFocus, "Official display text was not keyboard focusable.");
    const postTab = await keyAndSummarize("Tab");
    assert(
      postTab.tag !== null,
      "Official display text did not participate in keyboard navigation.",
    );
    result.keyboard = { display_focus: true, post_display_tab: postTab.tag };
    result.assertions.push("official_full_display_scroll_and_keyboard");
    await clickSelector(".official-profile-return");
    await waitFor(
      () => has("#official-profile-dialog").then((present) => !present),
      "official reading return topology",
    );
    // Select the DTO plan node through the backend node table when the
    // execution summary is available, otherwise through its graph card. The
    // graph, the table and the official node scope share one SelectionProvider.
    const tableLink = `Array.from(document.querySelectorAll('.workspace-doctor .plan-node-table .plan-node-link')).find((button) => button.querySelector('small')?.textContent?.trim() === ${JSON.stringify(`${nodeCandidate.plan_node_id}`)})`;
    const graphCard = `document.querySelector(${JSON.stringify(`.workspace-doctor .execution-graph .react-flow__node[data-id="${nodeCandidate.node_id}"]`)})`;
    const selectedCards = "Array.from(document.querySelectorAll('.workspace-doctor .execution-graph .plan-card.selected')).map((card) => card.dataset.nodeId).filter(Boolean)";
    const viaTable = await evaluate(`Boolean(${tableLink})`);
    result.node_selection_diagnostic = {
      target_node_sha256: hashOpaque(nodeCandidate.node_id),
      plan_node_id: nodeCandidate.plan_node_id,
      selection_control: viaTable ? "plan_node_table" : "graph_card",
    };
    try {
      await clickElement(
        viaTable ? tableLink : graphCard,
        "Current DTO plan node could not be selected.",
      );
      await waitFor(
        async () => {
          const selected = await evaluate(selectedCards);
          return selected.length === 1 && selected[0] === nodeCandidate.node_id && (await has(".workspace-doctor .execution-graph .node-inspector"));
        },
        "selected current DTO plan node",
      );
    } catch (error) {
      const selectedNodeIds = await evaluate(selectedCards);
      result.node_selection_diagnostic.actual_selected_node_sha256 =
        hashOpaqueIds(selectedNodeIds);
      result.node_selection_diagnostic.actual_selected_count =
        selectedNodeIds.length;
      result.node_selection_diagnostic.failure_phase =
        error instanceof Error ? error.message : String(error);
      throw error;
    }
    const identityAfterSelection = await evaluate(
      "window.__doctorE2eLastProfileFetchIdentity",
    );
    assert(
      sameOfficialProfileIdentitySummary(
        profileIdentity.identity,
        identityAfterSelection?.identity,
      ) &&
        identityAfterSelection?.discovery_generation ===
          profileIdentity.discovery_generation,
      "Selecting a plan node changed the current fetched Profile identity.",
    );
    assert(
      (await officialRequestLog()).length === wholeBefore + 1,
      "Selecting a plan node issued an official read.",
    );
    result.node_selection_diagnostic.actual_selected_count = 1;
    result.node_selection_diagnostic.identity_preserved_after_selection = true;
    result.assertions.push("selected_current_dto_plan_node_preserves_identity");
    await clickButton("Official reading");
    await waitFor(
      () => has("#official-profile-dialog"),
      "official node-scope dialog",
    );
    const nodeScopeButton = await evaluate(
      `document.querySelector('.official-profile-scope button:nth-child(2)')?.textContent?.trim() ?? ''`,
    );
    assert(
      nodeScopeButton === `Selected node [${nodeCandidate.plan_node_id}]`,
      "Official node scope label was not derived from the selected DTO plan node.",
    );
    await clickSelector(".official-profile-scope button:nth-child(2)");
    assert(
      (await officialRequestLog()).length === wholeBefore + 1,
      "Changing official scope issued a hidden request.",
    );
    const nodeBefore = (await officialRequestLog()).length;
    await clickButton("Read official analysis");
    await waitFor(async () => {
      const requests = await officialRequestLog();
      return (
        requests.length === nodeBefore + 1 && requests.at(-1)?.status === 200
      );
    }, "node-scope official response");
    await waitFor(
      async () =>
        (await officialResponseLog()).length >= 2 &&
        (await has(".official-profile-success")),
      "node-scope official text",
    );
    const nodeRequest = (await officialRequestLog()).at(-1);
    const nodeResponse = (await officialResponseLog()).at(-1);
    assertOfficialRequest(
      nodeRequest,
      [nodeCandidate.plan_node_id],
      profileIdentity,
    );
    assertOfficialResponse(
      nodeResponse,
      [nodeCandidate.plan_node_id],
      profileIdentity,
    );
    assertOfficialIdentityEcho(nodeRequest, nodeResponse);
    await recordOfficialSuccess(result, "plan_node", nodeResponse);
    result.assertions.push("node_scope_exact_dto_binding");
    if (officialProfileTransport === "controlled_mock_over_real_session") {
      await configureOfficialMockQueue([
        {
          kind: "typed_error",
          status: 503,
          payload: {
            code: "request_failed",
            stage: "request",
            message_id: "request_failed",
            recoverability: "retry_same",
            details: {},
          },
        },
        { kind: "native" },
      ]);
      const retryBefore = (await officialRequestLog()).length;
      await clickButton("Read again");
      await waitFor(
        async () =>
          (await officialRequestLog()).length === retryBefore + 1 &&
          (await has(".official-profile-failed")),
        "typed retry_same official failure",
      );
      const retryFailure = (await officialRequestLog()).at(-1);
      assert(
        retryFailure.status === 503,
        "Controlled retry_same response did not retain its typed status.",
      );
      assert(
        (await has(".official-profile-actions button")) &&
          (await textOf(".official-profile-actions button")) ===
            "Retry reading",
        "retry_same did not expose Retry reading.",
      );
      await clickButton("Retry reading");
      await waitFor(
        async () =>
          (await officialRequestLog()).length === retryBefore + 2 &&
          (await officialRequestLog()).at(-1)?.status === 200 &&
          (await has(".official-profile-success")),
        "explicit retry_same recovery",
      );
      result.recovery_retry_same = {
        failure_status: 503,
        explicit_retry: true,
        recovered: true,
      };
      result.assertions.push("typed_retry_same_explicit_recovery");
      await configureOfficialMockQueue([
        {
          kind: "typed_error",
          status: 422,
          payload: {
            code: "official_profile_unavailable",
            stage: "analyze_profile",
            message_id: "official_profile_empty",
            recoverability: "change_input",
            details: { limitation: "profile_empty" },
          },
        },
        { kind: "native" },
      ]);
      const unavailableBefore = (await officialRequestLog()).length;
      await clickButton("Read again");
      await waitFor(
        async () =>
          (await officialRequestLog()).length === unavailableBefore + 1 &&
          (await has(".official-profile-unavailable")),
        "typed official unavailable state",
      );
      assert(
        await textOf(".official-profile-error-details").then(
          (text) =>
            text.includes("official_profile_empty") &&
            text.includes("change_input"),
        ),
        "Typed official unavailable fields were not rendered.",
      );
      assert(
        (await textOf(".official-profile-actions button")) ===
          "Return to query list",
        "change_input did not expose Return to query list.",
      );
      result.recovery_change_input = {
        failure_status: 422,
        action: "return_to_query_list",
      };
      result.assertions.push("typed_change_input_recovery_action");
      await clickSelector(".official-profile-scope button:first-child");
      await waitFor(
        () =>
          has(".official-profile-action button") &&
          textOf(".official-profile-action button").then(
            (text) => text === "Read official analysis",
          ),
        "whole-range recovery after node unavailable",
      );
      const switched = await identitySwitchLateReplyCoverage(
        result,
        profileIdentity,
      );
      profileIdentity = switched.profileIdentity;
      await configureOfficialMockQueue([{ kind: "native" }]);
      await clickButton("Official reading");
      await waitFor(
        () =>
          has(
            '#official-profile-dialog[aria-label="Official profile reading"]',
          ),
        "official reading reopen after query identity switch",
      );
      const recoveryBefore = (await officialRequestLog()).length;
      const recoveryResponseBefore = (await officialResponseLog()).length;
      await clickButton("Read official analysis");
      await waitFor(
        async () =>
          (await officialRequestLog()).length === recoveryBefore + 1 &&
          (await officialRequestLog()).at(-1)?.status === 200,
        "whole-range recovery request after identity switch",
      );
      await waitFor(
        async () =>
          (await officialResponseLog()).length >= recoveryResponseBefore + 1 &&
          (await has(".official-profile-success")),
        "whole-range recovery success",
      );
      const recoveredRequest = (await officialRequestLog()).at(-1);
      const recoveredResponse = (await officialResponseLog()).at(-1);
      assertOfficialRequest(recoveredRequest, [], profileIdentity);
      assertOfficialResponse(recoveredResponse, [], profileIdentity);
      assertOfficialIdentityEcho(recoveredRequest, recoveredResponse);
      await configureOfficialMockQueue([
        {
          kind: "typed_error",
          status: 504,
          payload: {
            code: "remote_timeout",
            stage: "analyze_profile",
            message_id: "remote_timeout",
            recoverability: "restart",
            details: {},
          },
        },
      ]);
      await clickButton("Read again");
      await waitFor(
        () => has(".official-profile-failed"),
        "typed restart official failure",
      );
      assert(
        await textOf(".official-profile-error-details").then(
          (text) => text.includes("remote_timeout") && text.includes("restart"),
        ),
        "remote_timeout was not retained as restart.",
      );
      assert(
        (await textOf(".official-profile-actions button")) === "Reconnect",
        "restart did not expose Reconnect.",
      );
      result.recovery_restart = {
        failure_status: 504,
        action: "reconnect",
        recoverability_preserved: true,
      };
      result.assertions.push("typed_restart_reconnect_action");
    }
    const officialRequestsBeforeReturn = (await officialRequestLog()).length;
    await clickSelector(".official-profile-return");
    await waitFor(
      () => has("#official-profile-dialog").then((present) => !present),
      "official node-scope dialog closed before downstream lifecycle",
    );
    assert(
      (await officialRequestLog()).length === officialRequestsBeforeReturn,
      "Closing the node-scope official dialog issued a hidden request.",
    );
    result.assertions.push(
      "official_node_scope_dialog_closed_before_downstream_lifecycle",
    );
    result.request_counts.after = (await officialRequestLog()).length;
    result.request_counts.status_ok =
      result.request_counts.after > result.request_counts.before;
  } finally {
    if (mockInstalled) {
      const beforeRestore = await officialMockSnapshot();
      await restoreOfficialMock();
      const afterRestore = await officialMockSnapshot();
      result.mock_restored = Boolean(
        beforeRestore?.restored || afterRestore?.restored,
      );
      assert(
        result.mock_restored,
        "Controlled official mock did not restore native fetch in finally.",
      );
    }
  }
}
async function identitySwitchLateReplyCoverage(result, oldProfileIdentity) {
  await configureOfficialMockQueue([{ kind: "hold_native" }]);
  const before = (await officialRequestLog()).length;
  await clickButton("Read official analysis");
  await waitFor(async () => {
    const requests = await officialRequestLog();
    const mock = await officialMockSnapshot();
    return (
      requests.length === before + 1 &&
      requests.at(-1)?.status === null &&
      mock?.pending === true &&
      (await has('.official-profile-action[aria-busy="true"]'))
    );
  }, "held official response before query identity switch");
  const queryRoutesBeforeSwitch = (await routeTelemetry(auditOverviewRoute))
    .length;
  await clickButton("Queries");
  await waitFor(
    () => has(".workspace-queries .audit-workload"),
    "Audit history for identity switch",
  );
  await waitFor(async () => {
    const routes = await routeTelemetry(auditOverviewRoute);
    return (
      routes.length > queryRoutesBeforeSwitch &&
      routes.at(-1)?.method === "POST" &&
      routes.at(-1)?.status === 200 &&
      (await evaluate("document.querySelector('.workspace-queries .audit-workload')?.getAttribute('aria-busy') === 'false'"))
    );
  }, "Audit overview response for identity switch");
  await showAuditIndividualExecutions("query rankings response for identity switch");
  assert(
    (await evaluate("document.querySelectorAll('table[aria-label=\"Audited query rankings\"] tbody tr').length")) > 0,
    "Identity switch found no audited query rankings.",
  );
  await waitFor(
    async () => (await officialRequestLog()).at(-1)?.aborted === true,
    "old official request aborted on query identity switch",
  );
  const audit500ProjectionBefore = (
    await projectionTelemetry(auditOverviewRoute)
  ).length;
  const audit500RoutesBefore = (await routeTelemetry(auditOverviewRoute))
    .length;
  const identityAuditWindow = auditFixtureWindow ?? "1h";
  const identityAuditWindowSeconds =
    auditWindowSecondsByPreset[identityAuditWindow];
  await setInputValue('select[aria-label="Audit window"]', identityAuditWindow);
  const selectedIdentityWindow = await evaluate(
    "document.querySelector('select[aria-label=\"Audit window\"]')?.value ?? null",
  );
  assert(
    selectedIdentityWindow === identityAuditWindow,
    "Audit window control did not commit the retained-profile coverage window.",
  );
  await setInputValue('select[aria-label="Top limit"]', "500");
  assert(
    await evaluate(
      'document.querySelector(\'select[aria-label="Top limit"]\')?.value === "500"',
    ),
    "Top limit control did not commit 500 before Apply filters.",
  );
  await clickButton("Apply filters");
  await waitFor(async () => {
    const routes = await routeTelemetry(auditOverviewRoute);
    return (
      routes.length === audit500RoutesBefore + 1 &&
      routes.at(-1)?.method === "POST" &&
      routes.at(-1)?.status === 200 &&
      (await has('table[aria-label="Audited query rankings"]'))
    );
  }, "bounded 500-row Audit query response");
  const audit500Projection = await projectionAfterRoute(
    auditOverviewRoute,
    audit500ProjectionBefore,
    "500-row Audit query response",
  );
  const auditTop = audit500Projection?.audit_top;
  assert(
    audit500Projection?.audit_scope?.window_seconds ===
      identityAuditWindowSeconds,
    "Audit response scope did not retain the selected fixture coverage window.",
  );
  assert(
    auditTop?.requested_limit === 500,
    "Audit response did not retain the requested Top limit of 500.",
  );
  assert(
    Number.isInteger(auditTop?.returned_count) &&
      typeof auditTop?.truncated === "boolean" &&
      typeof auditTop?.full_window_count === "string",
    "Audit response omitted bounded returned/truncated/full-window evidence.",
  );
  const candidateRows = await evaluate(
    `(async () => {    const previousQueryIdSha256 = ${JSON.stringify(oldProfileIdentity.query_id_sha256)};    const sha256 = async (value) => Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(value))), (byte) => byte.toString(16).padStart(2, '0')).join('');    return await Promise.all(Array.from(document.querySelectorAll('table[aria-label="Audited query rankings"] tbody tr')).map(async (row, index) => {      const button = row.querySelector('button[aria-label^="Select audited query "]');      const queryId = button?.getAttribute('aria-label')?.replace(/^Select audited query\\s+/, '') ?? '';      const profile = row.cells.item(row.cells.length - 1)?.textContent?.trim() ?? '';      const query_id_sha256 = queryId ? await sha256(queryId) : null;      return { index, query_id_sha256, available: Boolean(button && !button.disabled && queryId && query_id_sha256 !== previousQueryIdSha256 && profile === 'Available') };    })).then((candidates) => candidates.filter((candidate) => candidate.available));  })()`,
  );
  const oldIdentityHash = oldProfileIdentity.query_id_sha256;
  const configuredAllowlist = retainedProfileHashAllowlist;
  const usableAllowlist = configuredAllowlist.filter(
    (hash) => hash !== oldIdentityHash,
  );
  const allowlistedCandidates = candidateRows.filter((candidate) =>
    usableAllowlist.includes(candidate.query_id_sha256),
  );
  result.identity_switch_audit_window = {
    fixture_window: identityAuditWindow,
    requested_limit: auditTop.requested_limit,
    returned: auditTop.returned_count,
    full_window_count: auditTop.full_window_count,
    truncated: auditTop.truncated,
    scope: audit500Projection.audit_scope,
  };
  result.identity_switch_profile_fixture = {
    allowlist_env: "E2E_RETAINED_PROFILE_HASHES",
    allowlist_configured: configuredAllowlist.length > 0,
    allowlist_sha256: configuredAllowlist,
    old_identity_excluded: configuredAllowlist.includes(oldIdentityHash),
    candidate_rows_available: candidateRows.length,
    allowlisted_candidate_rows: allowlistedCandidates.length,
    fetch_attempts_without_allowlist: 0,
  };
  assert(
    configuredAllowlist.length > 0,
    "INCONCLUSIVE: E2E_RETAINED_PROFILE_HASHES is not configured; no Profile fetch attempted.",
  );
  assert(
    allowlistedCandidates.length > 0,
    "INCONCLUSIVE: retained Profile SHA allowlist did not occur in the bounded 500-row Audit DOM; no Profile fetch attempted.",
  );
  const profileFetchAttempts = [];
  let selectedCandidate = null;
  for (const candidate of allowlistedCandidates) {
    await clickElement(
      `document.querySelectorAll('table[aria-label="Audited query rankings"] tbody tr')[${candidate.index}]?.querySelector('button[aria-label^="Select audited query "]')`,
      "An available query identity could not be selected.",
    );
    await waitFor(
      () =>
        evaluate(
          `(() => {      const row = document.querySelectorAll('table[aria-label="Audited query rankings"] tbody tr')[${candidate.index}];      const select = row?.querySelector('button[aria-label^="Select audited query "]');      const fetch = Array.from(document.querySelectorAll('.audit-selection-actions button')).find((button) => button.textContent.trim() === 'Diagnose');      return select?.getAttribute('aria-pressed') === 'true' && Boolean(fetch && !fetch.disabled);    })()`,
        ),
      "selected available query identity with Profile available",
    );
    const profileFetchBefore = (await routeTelemetry("/profiles/fetch")).length;
    const profileProjectionBefore = (
      await projectionTelemetry("/profiles/fetch")
    ).length;
    const profileErrorBefore = (await profileFetchErrors()).length;
    await clickElement(
      `Array.from(document.querySelectorAll('.audit-selection-actions button')).find(button => button.textContent.trim() === 'Diagnose' && !button.disabled)`,
      "Selected Audit Profile diagnose action was unavailable.",
    );
    await waitFor(async () => {
      const routes = await routeTelemetry("/profiles/fetch");
      const errors = await profileFetchErrors();
      const projections = await projectionTelemetry("/profiles/fetch");
      const route = routes.at(-1);
      return (
        routes.length === profileFetchBefore + 1 &&
        route?.method === "POST" &&
        (route.status === 200
          ? projections.length > profileProjectionBefore
          : errors.length > profileErrorBefore || route.status === 0)
      );
    }, "second query Profile fetch typed response");
    const profileFetchRoutes = await routeTelemetry("/profiles/fetch");
    const profileFetchProjections =
      await projectionTelemetry("/profiles/fetch");
    const profileFetchErrorRecords = await profileFetchErrors();
    const profileFetchRoute = profileFetchRoutes.at(-1);
    const profileFetchProjection =
      profileFetchProjections.length > profileProjectionBefore
        ? profileFetchProjections.at(-1)
        : null;
    const profileFetchError =
      profileFetchErrorRecords.length > profileErrorBefore
        ? profileFetchErrorRecords.at(-1)
        : null;
    profileFetchAttempts.push({
      row_index: candidate.index,
      query_id_sha256: candidate.query_id_sha256,
      status: profileFetchRoute?.status ?? null,
      state: profileFetchProjection?.profile_fetch_state ?? null,
      error_code: profileFetchError?.code ?? null,
      message_id: profileFetchError?.message_id ?? null,
      recoverability: profileFetchError?.recoverability ?? null,
    });
    if (
      profileFetchRoute?.status === 200 &&
      profileFetchProjection?.profile_fetch_state === "fetched"
    ) {
      await waitFor(
        () => has(".audit-profile-receipt"),
        "second query Profile fetch receipt",
      );
      selectedCandidate = candidate;
      break;
    }
  }
  result.identity_switch_profile_fetch_attempts = profileFetchAttempts;
  result.identity_switch_profile_candidate_count = candidateRows.length;
  result.identity_switch_profile_fixture.fetch_attempts_without_allowlist = 0;
  assert(
    selectedCandidate,
    `No existing Profile was fetchable from the ${profileFetchAttempts.length} allowlisted Available query candidates.`,
  );
  // One Diagnose click fetches the existing Profile and opens it in Doctor; no second action.
  await waitFor(
    () => has(".workspace-doctor .official-profile-open"),
    "Doctor reopened for second query identity",
  );
  await waitFor(
    () =>
      evaluate(
        `window.__doctorE2eLastProfileFetchIdentity?.query_id_sha256 && window.__doctorE2eLastProfileFetchIdentity.query_id_sha256 !== ${JSON.stringify(oldProfileIdentity.query_id_sha256)}`,
      ),
    "second query identity telemetry",
  );
  const nextProfileIdentity = await evaluate(
    "window.__doctorE2eLastProfileFetchIdentity",
  );
  assert(
    nextProfileIdentity?.query_id_sha256 &&
      nextProfileIdentity.query_id_sha256 !==
        oldProfileIdentity.query_id_sha256,
    "Queries UI did not rotate the current Profile identity.",
  );
  assert(
    nextProfileIdentity.connection_id_sha256 ===
      oldProfileIdentity.connection_id_sha256,
    "Query identity switch unexpectedly changed the connection session.",
  );
  await releaseOfficialMock();
  await wait(250);
  const lateRequest = (await officialRequestLog()).at(-1);
  assert(
    lateRequest?.aborted === true && lateRequest.status === 200,
    "Held official response did not arrive late after query identity rotation.",
  );
  assert(
    !(await has(".official-profile-success")) &&
      !(await has(".official-profile-dialog")),
    "Late official response repopulated the prior Doctor panel after query identity rotation.",
  );
  result.identity_switch_late_reply = {
    transport: "controlled_mock_over_real_session",
    identity_switch: "query_ui",
    old_request_aborted: true,
    late_native_response_status: 200,
    stale_success_dropped: true,
    new_profile_identity_admitted: true,
    new_official_request_count: 0,
    profile_fetch_attempts: profileFetchAttempts,
  };
  result.assertions.push(
    "query_identity_switch_aborts_and_drops_late_official_reply",
  );
  return { profileIdentity: nextProfileIdentity };
}
async function installOfficialMock() {
  await evaluate(
    `(() => {    if (window.__doctorE2eOfficialMockControl) return;    const previous = window.fetch;    const control = { queue: [], requests: [], pending: null, restored: false };    const parseBody = (init) => {      if (typeof init?.body !== 'string') return null;      try { return JSON.parse(init.body); } catch { return null; }    };    const isOfficial = (input) => window.__doctorE2eOfficialConnectionId?.(new URL(input instanceof Request ? input.url : String(input), window.location.href).pathname) !== null;    const recordRequest = async (input, init) => {      const url = new URL(input instanceof Request ? input.url : String(input), window.location.href);      const signal = init?.signal ?? (input instanceof Request ? input.signal : null);      const method = (init?.method ?? (input instanceof Request ? input.method : 'GET')).toUpperCase();      const summary = await window.__doctorE2eOfficialRequestSummary?.(url.pathname, method, parseBody(init));      const record = { ...(summary ?? { endpoint: 'invalid_official_analysis', path_connection_id_sha256: null, body: null }), method, status: null, aborted: Boolean(signal?.aborted) };      control.requests.push(record);      window.__doctorE2eOfficialRequests.push(record);      signal?.addEventListener('abort', () => { record.aborted = true; }, { once: true });      return record;    };    const forward = (input, init, keepSignal) => ({ ...(init ?? {}), signal: keepSignal ? (init?.signal ?? (input instanceof Request ? input.signal : undefined)) : undefined, __doctorE2eOfficialMockBypass: true });    const mockedFetch = async (input, init) => {      if (!isOfficial(input)) return previous(input, init);      const record = await recordRequest(input, init);      const action = control.queue.shift() ?? { kind: 'native' };      if (action.kind === 'typed_error') {        record.status = action.status ?? 503;        return new Response(JSON.stringify(action.payload), { status: record.status, headers: { 'Content-Type': 'application/json' } });      }      const responsePromise = previous(input, forward(input, init, action.kind !== 'hold_native'));      const response = await responsePromise;      if (action.kind === 'hold_native') {        await new Promise((resolve) => { control.pending = { resolve, record }; });        control.pending = null;      }      record.status = response.status;      if (response.ok) void response.clone().json()        .then((body) => window.__doctorE2eOfficialResponseSummary?.(body))        .then((summary) => { if (summary) window.__doctorE2eOfficialResponses.push(summary); })        .catch(() => undefined);      return response;    };    control.setQueue = (items) => { control.queue = Array.isArray(items) ? items.slice() : []; };    control.release = () => { const pending = control.pending; if (pending) pending.resolve(); };    control.restore = () => { if (window.fetch === mockedFetch) window.fetch = previous; control.restored = true; };    control.snapshot = () => ({ restored: control.restored, pending: Boolean(control.pending), request_count: control.requests.length });    window.__doctorE2eOfficialMockControl = control;    window.fetch = mockedFetch;  })()`,
  );
}
async function configureOfficialMockQueue(queue) {
  await evaluate(
    `window.__doctorE2eOfficialMockControl?.setQueue(${JSON.stringify(queue)})`,
  );
}
async function releaseOfficialMock() {
  await evaluate("window.__doctorE2eOfficialMockControl?.release()");
}
async function restoreOfficialMock() {
  await evaluate("window.__doctorE2eOfficialMockControl?.restore()");
}
async function officialMockSnapshot() {
  return evaluate("window.__doctorE2eOfficialMockControl?.snapshot() ?? null");
}
async function officialRequestLog() {
  return evaluate(
    "(window.__doctorE2eOfficialRequests ?? []).map((request) => ({ ...request, body: request.body ? { ...request.body, identity: request.body.identity ? { ...request.body.identity } : null, node_ids: Array.isArray(request.body.node_ids) ? [...request.body.node_ids] : request.body.node_ids } : null }))",
  );
}
async function officialResponseLog() {
  return evaluate(
    "(window.__doctorE2eOfficialResponses ?? []).map((response) => ({ ...response, identity: response.identity ? { ...response.identity } : null, node_ids: Array.isArray(response.node_ids) ? [...response.node_ids] : response.node_ids }))",
  );
}
function assertOfficialRequest(request, expectedNodeIds, profileIdentity) {
  assert(
    request?.method === "POST" && request.status === 200,
    "Official read did not complete as a successful POST.",
  );
  const body = request.body;
  assert(
    request.endpoint === "official_analysis" &&
      request.path_connection_id_sha256 ===
        profileIdentity.connection_id_sha256,
    "Official read used a different endpoint or connection.",
  );
  assert(
    body?.connection_id_sha256 === profileIdentity.connection_id_sha256 &&
      body.query_id_sha256 === profileIdentity.query_id_sha256,
    "Official body did not use the current fetched Profile query identity.",
  );
  assert(
    sameOfficialProfileIdentitySummary(body.identity, profileIdentity.identity),
    "Official body did not use the current tagged Profile identity.",
  );
  assert(
    JSON.stringify(body.node_ids) === JSON.stringify(expectedNodeIds),
    `Official body node scope drifted: ${JSON.stringify(body.node_ids)}.`,
  );
}
function assertOfficialResponse(response, expectedNodeIds, profileIdentity) {
  assert(
    response?.kind === "analyze_profile_text",
    "Official response kind was not analyze_profile_text.",
  );
  assert(
    response.connection_id_sha256 === profileIdentity.connection_id_sha256 &&
      response.query_id_sha256 === profileIdentity.query_id_sha256,
    "Official response query did not match the fetched Profile.",
  );
  assert(
    sameOfficialProfileIdentitySummary(
      response.identity,
      profileIdentity.identity,
    ),
    "Official response identity did not match the fetched tagged identity.",
  );
  assert(
    profileIdentity.identity.kind === "discovery"
      ? response.discovery_generation ===
          profileIdentity.identity.discovery_generation
      : response.discovery_generation === null,
    "Official response query generation did not match its identity arm.",
  );
  assert(
    JSON.stringify(response.node_ids) === JSON.stringify(expectedNodeIds),
    "Official response node scope did not match its request.",
  );
  assert(
    response.display_text_present === true &&
      response.text_present === true &&
      typeof response.display_hash === "string" &&
      typeof response.source_hash === "string",
    "Official success omitted display/raw fields.",
  );
}
function assertOfficialIdentityEcho(request, response) {
  assert(
    officialProfileIdentityEchoes(request?.body, response),
    "Official response did not echo the exact tagged request identity.",
  );
}
async function recordOfficialSuccess(result, scope, response) {
  const displayText = await textOf(".official-profile-display-text");
  assert(
    hashOpaque(displayText) === response.display_hash,
    "Official display text was truncated or changed in the DOM.",
  );
  const dimensions = await evaluate(
    `(() => { const element = document.querySelector('.official-profile-display-text'); return element ? { scroll_height: element.scrollHeight, client_height: element.clientHeight, scroll_width: element.scrollWidth, client_width: element.clientWidth } : null; })()`,
  );
  assert(
    dimensions &&
      (dimensions.scroll_height > dimensions.client_height ||
        dimensions.scroll_width > dimensions.client_width),
    "Official display text did not expose a scrollable full-text region.",
  );
  assert(
    await has(
      '.official-profile-raw-source pre[aria-label="Raw official text"]',
    ),
    "Official raw source artifact was not rendered separately.",
  );
  result.responses.push({
    scope,
    status: 200,
    display_hash: response.display_hash,
    source_hash: response.source_hash,
    display_length: displayText.length,
    raw_display_distinct: response.raw_display_distinct,
  });
}
async function keyAndSummarize(keyName) {
  await key(keyName);
  return activeSummary();
}
async function auditFilterCoverage() {
  const occurrenceBefore = (await projectionTelemetry(auditOverviewRoute))
    .length;
  if (!(await evaluate(`Array.from(document.querySelectorAll(".audit-view-switch button")).some((button) => button.textContent.trim() === "Individual executions")`)))
    await clickButton("Back to the list");
  await clickButton("Individual executions");
  const baseline = await projectionAfterRoute(
    auditOverviewRoute,
    occurrenceBefore,
    "Audit filter baseline",
  );
  const originalWindowPreset = await evaluate(
    "document.querySelector('select[aria-label=\"Audit window\"]')?.value ?? null",
  );
  const originalWindowSeconds =
    auditWindowSecondsByPreset[originalWindowPreset];
  assert(
    Number.isInteger(originalWindowSeconds) &&
      baseline.audit_scope?.window_seconds === originalWindowSeconds &&
      baseline.audit_scope.state === null &&
      !baseline.audit_scope.has_user_filter,
    "Family return did not restore the original applied Audit scope.",
  );
  assert(
    await has('input[aria-label="Exact user filter"]'),
    "Exact user filter control was not rendered.",
  );
  const userAvailable = await evaluate(
    `document.querySelector('input[aria-label="Exact user filter"]').disabled === false`,
  );
  if (userAvailable)
    await setInputValue(
      'input[aria-label="Exact user filter"]',
      "__doctor_e2e_absent_user__",
    );
  else
    assert(
      await textOf("#audit-user-filter-capability").then(
        (text) => text.length > 0,
      ),
      "Disabled user filtering did not explain its capability state.",
    );
  await setInputValue('select[aria-label="Audit source state"]', "error");
  await setInputValue('select[aria-label="Audit window"]', "30d");
  assert(
    await textOf(".audit-filter-bar").then((text) =>
      text.includes("30-day read may cover every retained partition"),
    ),
    "30d audit window did not expose its bounded-partition note.",
  );
  const executionBefore = (await routeTelemetry("/runtime-profile")).length;
  const sqlBefore = (await routeTelemetry("/audit/sql")).length;
  const familyBefore = (await projectionTelemetry(auditOverviewRoute)).length;
  // A digest-free window hides the family tab; switching back to individual
  // executions reloads with the same applied scope and proves the same rule.
  const familiesOffered = await evaluate(
    `Array.from(document.querySelectorAll(".audit-view-switch button")).some((button) => button.textContent.trim() === "Repeated SQL")`,
  );
  const switchLabel = familiesOffered ? "Repeated SQL" : "Individual executions";
  await clickButton(switchLabel);
  const unapplied = await projectionAfterRoute(
    auditOverviewRoute,
    familyBefore,
    "Unapplied filter tab switch",
  );
  assert(
    JSON.stringify(unapplied.audit_scope) ===
      JSON.stringify(baseline.audit_scope),
    "Tab switch silently applied part of the filter draft.",
  );
  await waitFor(
    () =>
      textOf(".audit-window-evidence").then((text) =>
        text.includes("Filters changed. Apply them to update the results."),
      ),
    "visible unapplied filter draft",
  );
  const applyBefore = (await projectionTelemetry(auditOverviewRoute)).length;
  await clickButton("Apply filters");
  const applied = await projectionAfterRoute(
    auditOverviewRoute,
    applyBefore,
    "Explicit filter apply",
  );
  assert(
    applied.audit_scope?.window_seconds === 2592000 &&
      applied.audit_scope.state === "error" &&
      applied.audit_scope.has_user_filter === userAvailable,
    "Explicit Apply did not use the complete user/state/window scope.",
  );
  await waitFor(
    () =>
      textOf(".audit-window-evidence").then(
        (text) => !text.includes("Filters changed. Apply them to update the results."),
      ),
    "applied filter result scope",
  );
  assert(
    (await routeTelemetry("/runtime-profile")).length === executionBefore &&
      (await routeTelemetry("/audit/sql")).length === sqlBefore,
    "Audit filtering executed or read a query SQL body.",
  );
  receipt.audit_filters = {
    user_control: userAvailable ? "available_tested" : "disabled_with_reason",
    tab_switch: switchLabel,
    baseline: baseline.audit_scope,
    applied: applied.audit_scope,
  };
  receipt.assertions.push("audit_filter_draft_tab_switch_and_explicit_apply");
  if (userAvailable)
    await setInputValue('input[aria-label="Exact user filter"]', "");
  await setInputValue('select[aria-label="Audit source state"]', "");
  await setInputValue(
    'select[aria-label="Audit window"]',
    originalWindowPreset,
  );
  assert(
    (await evaluate(
      "document.querySelector('select[aria-label=\"Audit window\"]')?.value ?? null",
    )) === originalWindowPreset,
    "Audit filter reset did not restore the captured fixture window control.",
  );
  const resetBefore = (await projectionTelemetry(auditOverviewRoute)).length;
  await clickButton("Apply filters");
  const reset = await projectionAfterRoute(
    auditOverviewRoute,
    resetBefore,
    "Audit filter reset",
  );
  assert(
    JSON.stringify(reset.audit_scope) === JSON.stringify(baseline.audit_scope),
    "Audit filter reset changed the original scope dimensions.",
  );
  receipt.audit_filters = {
    ...receipt.audit_filters,
    original_window_preset: originalWindowPreset,
    original_window_seconds: originalWindowSeconds,
    reset: reset.audit_scope,
  };
}
async function connectFixtureCoverage() {
  const fixedSqlFixture =
    project === "connected-sql-docker" || project === "connected-audit-docker";
  if (fixedSqlFixture)
    assert(
      process.env.E2E_FIXED_DOCKER_VALIDATED === "3514-loopback",
      "Focused SQL E2E requires the fixed Docker validation wrapper.",
    );
  const password = fixedSqlFixture
    ? ""
    : (process.env.E2E_DB_PASSWORD ??
      deriveFixturePassword(
        requiredEnv("STARROCKS_E2E_SECRET"),
        "doctor_reader",
      ));
  await clickButton("Connect");
  await waitFor(
    () => has('[role="dialog"]'),
    "connection dialog before connected E2E",
  );
  // Each input is the sole input child of its label, so CSS nth-of-type would
  // select within a label rather than across the dialog form. Index the dialog
  // input collection explicitly to exercise the real host/port/user/password
  // controls without depending on visual layout.
  await setInputAt('[role="dialog"] input', 0, "127.0.0.1");
  await setInputAt(
    '[role="dialog"] input',
    1,
    fixedSqlFixture ? "39230" : (process.env.E2E_DB_PORT ?? "39130"),
  );
  await setInputAt(
    '[role="dialog"] input',
    2,
    fixedSqlFixture ? "root" : "doctor_reader",
  );
  await setInputAt('[role="dialog"] input', 3, password);
  await clickButton("Connect read-only");
  await waitFor(
    () => has('[role="dialog"] .connection-state.success'),
    "read-only connection",
  );
  const connectionState = await textOf(
    '[role="dialog"] .connection-state.success',
  );
  assert(
    connectionState.startsWith("Connected") && connectionState.includes("3.5.14"),
    "Connected browser E2E did not create an exact supported session.",
  );
  // The current dialog exposes supported version and state, not session tokens.
  // Keep the receipt redacted while verifying the exact supported version.
  receipt.connected_connection_state = "ready";
  // The session remains live when an operator dismisses its successful probe;
  // do not click a background workspace through the modal backdrop.
  await clickButton("Close");
  await waitFor(
    () => has('[role="dialog"]').then((present) => !present),
    "connected dialog close",
  );
  receipt.assertions.push("connected_connection_dialog_close_retains_session");
  receipt.connected_fixture_service = {
    declared_project: project,
    candidate_sql_scope: "isolated_starrocks_fixture_information_schema",
    plan_sql_scope:
      project === "connected-audit-docker"
        ? "not_executed_existing_audit_only"
        : fixedSqlFixture
          ? "fixed_complex_four_tables_three_joins"
          : "isolated_starrocks_fixture_information_schema",
  };
}
/** * Task49 is deliberately an alternate connected-sql run, not an addition to * the normal SQL E2E.  The real fixture connection and visible controls are * still used, but the one selected POST is answered inside the browser before * it can reach the API/StarRocks.  Each Runtime terminal-unknown scenario gets * its own real session; a second Runtime POST is never attempted in an already * locked session. */ async function runtimeUnknownGuardCoverage() {
  assert(
    project === "connected-sql-docker" &&
      runtimeUnknownGuardTransport === "controlled_mock_over_real_session",
    "Task49 Runtime unknown guard must use its explicit connected-sql controlled transport.",
  );
  const result = {
    transport: "controlled_mock_over_real_session",
    ordinary_explain_timeout: null,
    scenarios: [],
  };
  receipt.runtime_unknown_guard = result;
  let priorConnectionId = null;
  for (const scenario of ["runtime_abort", "runtime_timeout"]) {
    await connectFixtureCoverage();
    try {
      await openQueryMode("SQL workspace");
      await waitFor(
        () => has(".workspace-queries .sql-analysis-workspace"),
        `Task49 plan input for ${scenario}`,
      );
      const connectionId = await currentControlledFixtureConnectionId();
      assert(
        connectionId !== priorConnectionId,
        "Task49 unknown scenarios must use separate real fixture sessions.",
      );
      if (scenario === "runtime_abort") {
        result.ordinary_explain_timeout =
          await ordinaryExplainTimeoutDoesNotLockRuntime(connectionId);
      }
      result.scenarios.push(
        await controlledRuntimeUnknownScenario(connectionId, scenario),
      );
      priorConnectionId = connectionId;
    } finally {
      await closeControlledFixtureSession();
    }
  }
  assert(
    result.ordinary_explain_timeout?.runtime_submit_enabled === true,
    "A read-only Explain timeout locked Runtime submission.",
  );
  assert(
    result.scenarios.length === 2 &&
      result.scenarios.every(
        (scenario) =>
          scenario.same_session_locked &&
          scenario.doctor_queries_round_trip_locked &&
          scenario.second_post_blocked &&
          scenario.mock_restored &&
          scenario.target_forwarded_to_database === false,
      ),
    "Task49 Runtime unknown guard did not preserve every terminal same-session lock.",
  );
  receipt.assertions.push(
    "task49_controlled_runtime_unknown_abort_timeout_new_sessions_no_target_sql_rerun",
  );
  receipt.assertions.push(
    "task49_controlled_explain_timeout_keeps_runtime_available",
  );
}
async function currentControlledFixtureConnectionId() {
  await clickButton("Manage connection");
  await waitFor(
    () => has('[role="dialog"] .connection-state.success'),
    "Task49 real fixture session state",
  );
  const connectionId = await evaluate(
    `(() => {    const value = document.querySelector('[role="dialog"] .connection-state.success strong')?.textContent ?? '';    const match = /^ready:\\s*([^\\s]+)$/.exec(value.trim());    return match?.[1] ?? null;  })()`,
  );
  await clickButton("Close");
  await waitFor(
    () => has('[role="dialog"]').then((present) => !present),
    "Task49 connection dialog close",
  );
  assert(
    typeof connectionId === "string" && connectionId.length > 0,
    "Task49 could not bind the controlled interceptor to the current fixture session.",
  );
  return connectionId;
}
async function closeControlledFixtureSession() {
  if (!(await has('button[aria-haspopup="dialog"]'))) return;
  await clickButton("Manage connection");
  await waitFor(
    () => has('[role="dialog"]'),
    "Task49 connection dialog before session close",
  );
  const closeAvailable = await evaluate(
    `Array.from(document.querySelectorAll('[role="dialog"] button')).some((button) => button.textContent.trim() === 'Disconnect' && !button.disabled)`,
  );
  if (!closeAvailable) {
    await clickButton("Close");
    await waitFor(
      () => has('[role="dialog"]').then((present) => !present),
      "Task49 unavailable connection dialog close",
    );
    return;
  }
  await clickButton("Disconnect");
  await waitFor(
    () =>
      textOf('[role="dialog"] .connection-state').then((text) =>
        text.includes(
          "Connection closed; credentials were removed from the server session.",
        ),
      ),
    "Task49 real fixture session close",
  );
  await clickButton("Close");
  await waitFor(
    () => has('[role="dialog"]').then((present) => !present),
    "Task49 closed session dialog close",
  );
}
async function ordinaryExplainTimeoutDoesNotLockRuntime(connectionId) {
  const explainBefore = (await routeTelemetry("/explain")).length;
  const result = {
    transport: "controlled_mock_over_real_session",
    injected_status: 504,
    explain_routes_added: 0,
    runtime_submit_enabled: false,
    target_forwarded_to_database: null,
    mock_restored: false,
  };
  await installRuntimeUnknownGuardMock(connectionId, "explain_timeout");
  try {
    await clickElement(
      `document.querySelector('[data-plan-mode=costs]')`,
      "SQL workspace control unavailable.",
    );
    await setInputValue("[data-sql-analysis-input=sql]", isolatedFixtureSql);
    await clickElement(
      `document.querySelector('[data-sql-analysis-action=view-plan]')`,
      "View plan unavailable.",
    );
    await waitFor(async () => {
      const routes = await routeTelemetry("/explain");
      return (
        routes.length === explainBefore + 1 &&
        routes.at(-1)?.status === 504 &&
        (await textOf("[data-sql-analysis-plan-result=error] code").then(
          (text) => text === "remote_timeout",
        ))
      );
    }, "Task49 controlled read-only Explain timeout");
    const runtime = await runtimeUnknownGuardState();
    assert(
      runtime.disabled === false && runtime.error === "remote_timeout",
      "A read-only Explain timeout disabled Runtime submission.",
    );
    const mock = await runtimeUnknownGuardMockSnapshot();
    assert(
      mock?.intercepted === 1 && mock.target_forwarded_to_database === false,
      "Task49 controlled Explain route did not remain an exact local intercept.",
    );
    result.explain_routes_added = 1;
    result.runtime_submit_enabled = true;
    result.target_forwarded_to_database = false;
  } finally {
    const beforeRestore = await runtimeUnknownGuardMockSnapshot();
    await restoreRuntimeUnknownGuardMock();
    const afterRestore = await runtimeUnknownGuardMockSnapshot();
    result.mock_restored = Boolean(
      beforeRestore?.restored || afterRestore?.restored,
    );
    assert(
      result.mock_restored,
      "Task49 controlled Explain timeout did not restore fetch in finally.",
    );
  }
  return result;
}
async function controlledRuntimeUnknownScenario(connectionId, scenario) {
  const expectedStatus = scenario === "runtime_abort" ? 0 : 504;
  const runtimeBefore = (await routeTelemetry("/runtime-profile")).length;
  const submissionsBefore = await evaluate(
    "window.__doctorE2eRuntimeSubmissions.length",
  );
  const result = {
    scenario,
    transport: "controlled_mock_over_real_session",
    injected_status: expectedStatus,
    runtime_routes_added: 0,
    same_session_locked: false,
    doctor_queries_round_trip_locked: false,
    second_post_blocked: false,
    target_forwarded_to_database: null,
    mock_restored: false,
  };
  await installRuntimeUnknownGuardMock(connectionId, scenario);
  try {
    await setInputValue("[data-sql-analysis-input=sql]", isolatedFixtureSql);
    await confirmStandardExecution();
    await waitFor(async () => {
      const routes = await routeTelemetry("/runtime-profile");
      const state = await runtimeUnknownGuardState();
      return (
        routes.length === runtimeBefore + 1 &&
        routes.at(-1)?.status === expectedStatus &&
        state.disabled === true &&
        state.stage === "unknown"
      );
    }, `Task49 controlled ${scenario} unknown lock`);
    assert(
      (await evaluate("window.__doctorE2eRuntimeSubmissions.length")) ===
        submissionsBefore + 1,
      "Task49 first Runtime click did not produce exactly one controlled submission.",
    );
    const firstState = await runtimeUnknownGuardState();
    const clickDisabled = await evaluate(
      `!document.querySelector('[data-sql-analysis-action=request-confirmation], [data-sql-analysis-action=confirm-execution]')`,
    );
    assert(
      clickDisabled && firstState.execution_action_count === 0,
      "Task49 unknown Runtime action was not visibly disabled.",
    );
    await wait(100);
    assert(
      (await routeTelemetry("/runtime-profile")).length === runtimeBefore + 1,
      "Task49 disabled Runtime action produced a second POST.",
    );
    await clickButton("Doctor");
    await waitFor(
      () => has(".workspace-doctor"),
      `Task49 Doctor round trip for ${scenario}`,
    );
    await clickButton("Queries");
    await waitFor(
      () => has(".workspace-queries .sql-analysis-workspace"),
      `Task49 Queries return for ${scenario}`,
    );
    const returnedState = await runtimeUnknownGuardState();
    assert(
      returnedState.disabled === true && returnedState.stage === "unknown",
      "Task49 Queries/Doctor navigation released the same-session Runtime lock.",
    );
    const returnedClickDisabled = await evaluate(
      `!document.querySelector('[data-sql-analysis-action=request-confirmation], [data-sql-analysis-action=confirm-execution]')`,
    );
    assert(
      returnedClickDisabled && returnedState.execution_action_count === 0,
      "Task49 returned Runtime action was not visibly disabled.",
    );
    await wait(100);
    assert(
      (await routeTelemetry("/runtime-profile")).length === runtimeBefore + 1,
      "Task49 returned disabled Runtime action produced a second POST.",
    );
    const mock = await runtimeUnknownGuardMockSnapshot();
    assert(
      mock?.intercepted === 1 && mock.target_forwarded_to_database === false,
      "Task49 controlled Runtime route did not remain an exact local intercept.",
    );
    result.runtime_routes_added = 1;
    result.same_session_locked =
      firstState.disabled && firstState.stage === "unknown";
    result.doctor_queries_round_trip_locked =
      returnedState.disabled && returnedState.stage === "unknown";
    result.second_post_blocked = true;
    result.target_forwarded_to_database = false;
  } finally {
    const beforeRestore = await runtimeUnknownGuardMockSnapshot();
    await restoreRuntimeUnknownGuardMock();
    const afterRestore = await runtimeUnknownGuardMockSnapshot();
    result.mock_restored = Boolean(
      beforeRestore?.restored || afterRestore?.restored,
    );
    assert(
      result.mock_restored,
      `Task49 controlled ${scenario} mock did not restore fetch in finally.`,
    );
  }
  return result;
}
async function runtimeUnknownGuardState() {
  return evaluate(
    `(() => {    const root = document.querySelector('.sql-analysis-workspace');    const stage = root?.getAttribute('data-sql-analysis-stage') ?? null;    const button = root?.querySelector('[data-sql-analysis-action=request-confirmation]');    const actions = root?.querySelectorAll('[data-sql-analysis-action=request-confirmation], [data-sql-analysis-action=confirm-execution]');    return { stage, disabled: stage === 'unknown' ? button === null || button === undefined : button?.disabled ?? null, error: root?.querySelector('[data-sql-analysis-plan-result=error] code')?.textContent?.trim() ?? null, execution_action_count: actions?.length ?? null };  })()`,
  );
}
async function confirmStandardExecution(dispatch = true) {
  const paths = ["/explain", "/runtime-profile"];
  const before = await Promise.all(
    paths.map(async (path) => (await routeTelemetry(path)).length),
  );
  const sql = await evaluate(
    `document.querySelector('[data-sql-analysis-input=sql]')?.value`,
  );
  await clickElement(
    `document.querySelector('[data-sql-analysis-action=request-confirmation]')`,
    "Execution review action unavailable.",
  );
  await waitFor(
    () =>
      has(
        ".sql-analysis-confirmation [data-sql-analysis-action=confirm-execution]",
      ),
    "Frozen SQL confirmation",
  );
  assert(
    (await evaluate(
      `document.querySelector('.sql-analysis-confirmation [data-sql-analysis-input=sql]')?.value`,
    )) === sql,
    "Confirmation changed the exact SQL draft.",
  );
  const after = await Promise.all(
    paths.map(async (path) => (await routeTelemetry(path)).length),
  );
  assert(
    after.every((count, index) => count === before[index]),
    "Review step executed SQL before explicit confirmation.",
  );
  if (dispatch)
    await clickElement(
      `document.querySelector('[data-sql-analysis-action=confirm-execution]')`,
      "Confirm execution action unavailable.",
    );
}
async function installRuntimeUnknownGuardMock(connectionId, scenario) {
  await evaluate(
    `(() => {    const prior = window.__doctorE2eRuntimeUnknownGuardControl;    if (prior && !prior.restored) throw new Error('Task49 controlled Runtime guard is already active.');    const previous = window.fetch;    const targetPath = "/api/v1/connections/" + encodeURIComponent(${JSON.stringify(connectionId)});    const endpoint = ${JSON.stringify(scenario)} === "explain_timeout" ? "explain" : "runtime-profile";    const status = ${JSON.stringify(scenario)} === "runtime_abort" ? 0 : 504;    const control = {      transport: "controlled_mock_over_real_session",      scenario: ${JSON.stringify(scenario)},      endpoint,      intercepted: 0,      target_forwarded_to_database: false,      restored: false,    };    const isExactTarget = (input, init) => {      const request = input instanceof Request ? input : null;      const url = new URL(request ? request.url : String(input), window.location.href);      const method = (init?.method ?? request?.method ?? "GET").toUpperCase();      return method === "POST" && url.pathname === targetPath + "/" + endpoint;    };    const mockedFetch = async (input, init) => {      if (!isExactTarget(input, init)) return previous(input, init);      control.intercepted += 1;      if (typeof window.__doctorE2eRecordControlledRoute !== "function") throw new Error('Task49 route telemetry is unavailable.');      window.__doctorE2eRecordControlledRoute(input, init, { status, runtime: endpoint === "runtime-profile" });      if (status === 0) throw new DOMException("Task49 controlled Runtime response abort", "AbortError");      const payload = {        code: "remote_timeout",        stage: endpoint === "runtime-profile" ? "runtime_execute" : "explain",        message_id: "remote_timeout",        recoverability: "restart",        details: {},      };      return new Response(JSON.stringify(payload), { status, headers: { "Content-Type": "application/json" } });    };    control.snapshot = () => ({      transport: control.transport,      scenario: control.scenario,      endpoint: control.endpoint,      intercepted: control.intercepted,      target_forwarded_to_database: control.target_forwarded_to_database,      restored: control.restored,    });    control.restore = () => {      if (window.fetch === mockedFetch) window.fetch = previous;      control.restored = true;      Object.defineProperty(window, "__doctorE2eRuntimeUnknownGuardResult", { configurable: true, value: control.snapshot() });      delete window.__doctorE2eRuntimeUnknownGuardControl;    };    Object.defineProperty(window, "__doctorE2eRuntimeUnknownGuardControl", { configurable: true, value: control });    window.fetch = mockedFetch;  })()`,
  );
}
async function runtimeUnknownGuardMockSnapshot() {
  return evaluate(
    "window.__doctorE2eRuntimeUnknownGuardControl?.snapshot() ?? window.__doctorE2eRuntimeUnknownGuardResult ?? null",
  );
}
async function restoreRuntimeUnknownGuardMock() {
  await evaluate("window.__doctorE2eRuntimeUnknownGuardControl?.restore()");
}
async function connectedReadonlyCoverage() {
  const baseline = requiredEnv("E2E_BASELINE_QUERY_ID");
  const candidate = requiredEnv("E2E_CANDIDATE_QUERY_ID");
  if (baseline === candidate)
    fail("Connected E2E requires two distinct query IDs.");
  await connectFixtureCoverage();
  // Discover retained workload Profiles before Audit acceptance produces its own
  // newer reader rows. Keep the product's real 100-row bound; do not rerun SQL.
  await openQueryMode("Profile list");
  await waitFor(
    () => has(".workspace-queries .query-table"),
    "existing Profiles / Compare workspace",
  );
  await waitFor(
    () =>
      evaluate(
        `Array.from(document.querySelectorAll('button')).some((item) => item.textContent.trim() === 'Refresh' && !item.disabled)`,
      ),
    "enabled query refresh",
  );
  // Clear the default finished-only filter so the controlled failed workload
  // row proves that unavailable Profile actions are visible and disabled.
  await setInputValue(".query-filters input", "");
  const discoveryRoutesBeforeRefresh = (await routeTelemetry("/queries/search"))
    .length;
  await clickButton("Refresh");
  // The auto-load already rendered the baseline row. Waiting for that same row
  // is therefore not enough to prove this explicit refresh committed its newer
  // discovery generation. Observe the HTTP completion and the re-enabled
  // control before selecting a row, otherwise a fetch can legitimately be
  // rejected as generation-expired while React still shows the prior list.
  await waitFor(async () => {
    const routes = await routeTelemetry("/queries/search");
    const refreshEnabled = await evaluate(
      `Array.from(document.querySelectorAll('button')).some((item) => item.textContent.trim() === 'Refresh' && !item.disabled)`,
    );
    return (
      routes.length > discoveryRoutesBeforeRefresh &&
      routes.at(-1)?.status === 200 &&
      refreshEnabled
    );
  }, "explicit query refresh completion");
  // The fixture AuditLoader schema proves isQuery/stmt, so the default
  // discovery request hides this tool's own statements and the page shows the
  // backend's exact hidden counts instead of silently dropping rows.
  const discoveryStatementFilter = await evaluate(
    `(() => { const node = document.querySelector('.query-statement-filter-evidence'); return { state: node?.dataset.queryStatementFilter ?? null, text: node?.textContent ?? '', checked: document.querySelector('.query-statement-filter input')?.checked ?? null }; })()`,
  );
  assert(
    discoveryStatementFilter.checked === true &&
      discoveryStatementFilter.state === "applied" &&
      /Hidden: \d+ statements sent by this tool and \d+ non-query statements/.test(
        discoveryStatementFilter.text,
      ),
    `Query discovery did not disclose an applied statement filter: ${JSON.stringify(discoveryStatementFilter)}`,
  );
  receipt.connected_discovery_statement_filter = discoveryStatementFilter;
  receipt.assertions.push("connected_discovery_hides_tool_statements_with_counts");
  await waitFor(
    () =>
      textOf(".query-table").then((text) =>
        text.includes("Query not finished"),
      ),
    "typed unavailable Profile row",
  );
  await clickElement(
    `Array.from(document.querySelectorAll('.query-table tbody tr')).find((row) => row.textContent.includes('Query not finished'))?.querySelector('button')`,
    "Unavailable Profile row could not be selected.",
  );
  const unavailableActionState = await evaluate(
    `({    fetch_disabled: Array.from(document.querySelectorAll('.selected-query-actions button')).find((item) => item.textContent.trim() === 'Fetch profile')?.disabled ?? false,    availability: Array.from(document.querySelectorAll('.selected-query dd')).map((item) => item.textContent.trim()).includes('Query not finished')  })`,
  );
  assert(
    unavailableActionState.fetch_disabled &&
      unavailableActionState.availability,
    "Unavailable Profile row exposed an executable or ambiguous action.",
  );
  receipt.connected_unavailable_profile_action = unavailableActionState;
  receipt.assertions.push("connected_unavailable_profile_action_disabled");
  await waitFor(
    () => has(`button[aria-label="Select query ${baseline}"]`),
    "baseline Audit/ProfileList row",
  );
  await clickSelector(`button[aria-label="Select query ${baseline}"]`);
  await clickButton("Fetch profile");
  // Definition-list text is intentionally compacted by textContent (for
  // example, `AvailabilityFetchedSource`), so a word-boundary scan is not a
  // reliable state assertion. Read the labelled DTO state instead.
  await waitFor(
    () =>
      Promise.all([selectedProfileAvailability(), textOf(".query-error")]).then(
        ([availability, error]) =>
          availability === "Fetched" || error.trim().length > 0,
      ),
    "baseline Profile fetch terminal state",
  );
  receipt.connected_profile_fetch = {
    availability: await selectedProfileAvailability(),
    error_message_id: (await textOf(".query-error")).trim() || null,
  };
  assert(
    receipt.connected_profile_fetch.error_message_id === null,
    `Baseline Profile fetch failed with ${receipt.connected_profile_fetch.error_message_id ?? "an unknown error"}.`,
  );
  assert(
    receipt.connected_profile_fetch.availability === "Fetched",
    `Baseline Profile fetch did not reach Fetched; got ${receipt.connected_profile_fetch.availability ?? "no availability state"}.`,
  );
  receipt.connected_profile_fetch_routes =
    await routeTelemetry("/profiles/fetch");
  assert(
    receipt.connected_profile_fetch_routes.some(
      (route) => route.method === "POST" && route.status === 200,
    ),
    "Browser fetch did not receive a successful connected Profile response.",
  );
  receipt.assertions.push("connected_query_select_fetch");
  await clickButton("Set baseline");
  await clickSelector(`button[aria-label="Select query ${candidate}"]`);
  await clickButton("Set candidate");
  await clickButton("Compare selected Profiles");
  await waitFor(
    () => has(".workspace-compare"),
    "connected Profile comparison",
  );
  receipt.connected_profile_compare_routes =
    await routeTelemetry("/profiles/compare");
  assert(
    receipt.connected_profile_compare_routes.some(
      (route) => route.method === "POST" && route.status === 200,
    ),
    "Browser comparison did not receive a successful connected Profile response.",
  );
  receipt.assertions.push("connected_two_profile_compare");
  // The real comparison page (design board 04) for the self-review; later
  // discovery changes retire it, so it is captured here.
  await waitFor(() => has(".workspace-compare .compare-structure"), "connected comparison rendered");
  await evaluate("window.scrollTo(0, 0)");
  await capture("connected-compare-1440x1024");
  await clickButton("Raw / Provenance");
  await waitFor(
    () => has(".compare-provenance-panel"),
    "connected comparison provenance",
  );
  const provenance = await textOf(".compare-provenance-panel");
  assert(
    /Analysis key|Facts hash|Source hash|No analyzed Profile selected/.test(
      provenance,
    ),
    "Compare provenance panel did not expose read-only DTO evidence.",
  );
  assert(
    !(await textOf("body")).includes("Not available in v0.1"),
    "Compare provenance was incorrectly marked unavailable in v0.1.",
  );
  receipt.assertions.push("connected_compare_raw_provenance_readonly");
  await auditWorkloadCoverage();
  await topologyAndRuntimeCoverage();
  // Keep the established final Compare screenshot and narrow-viewport evidence;
  // the dedicated screenshots above preserve both execution projections.
  await clickButton("Compare");
  await waitFor(
    () => has(".workspace-compare"),
    "Compare workspace after Runtime Top",
  );
}
async function sqlDraftRoundTripCoverage(mode, planMode) {
  const before = await evaluate(
    `document.querySelector('[data-sql-analysis-input=sql]')?.value ?? null`,
  );
  assert(
    typeof before === "string" && before.length > 0,
    "SQL draft must exist before workspace navigation.",
  );
  const paths = ["/explain", "/runtime-profile"];
  const counts = await Promise.all(
    paths.map(async (path) => (await routeTelemetry(path)).length),
  );
  await clickButton("Doctor");
  await waitFor(
    () => has(".workspace-doctor"),
    "Doctor during SQL draft round trip",
  );
  await clickButton("Queries");
  await waitFor(
    () => has(".workspace-queries [data-sql-analysis-input=sql]"),
    "SQL draft return from Doctor",
  );
  const after = await evaluate(
    `({    sql: document.querySelector('[data-sql-analysis-input=sql]')?.value ?? null,    planMode: document.querySelector('[data-plan-mode][aria-pressed=true]')?.getAttribute('data-plan-mode') ?? null,  })`,
  );
  assert(
    after.sql === before && after.planMode === planMode,
    "Workspace navigation lost SQL draft text or selected plan mode.",
  );
  const afterCounts = await Promise.all(
    paths.map(async (path) => (await routeTelemetry(path)).length),
  );
  assert(
    afterCounts.every((count, index) => count === counts[index]),
    "SQL draft navigation implicitly planned or executed a query.",
  );
  (receipt.sql_draft_round_trips ??= []).push({
    mode,
    character_count: before.length,
    text_preserved: true,
    zero_execution_requests: true,
  });
  receipt.assertions.push(`sql_draft_workspace_round_trip:${mode}`);
}
async function topologyAndRuntimeCoverage() {
  const entryPaths = ["/explain", "/runtime-profile"];
  const entryCounts = await Promise.all(entryPaths.map(async path => (await routeTelemetry(path)).length));
  if (!(await has(".workspace-queries .sql-analysis-workspace")))
    await openQueryMode("SQL workspace");
  await waitFor(
    () => has(".workspace-queries .sql-analysis-workspace"),
    "connected SQL workspace",
  );
  const enteredCounts = await Promise.all(entryPaths.map(async path => (await routeTelemetry(path)).length));
  assert(enteredCounts.every((count, index) => count === entryCounts[index]), "Opening SQL workspace implicitly planned or executed SQL.");
  const explainRoutesBefore = (await routeTelemetry("/explain")).length;
  const explainProjectionsBefore = (await projectionTelemetry("/explain"))
    .length;
  const runtimeRoutesBeforeTopology = (await routeTelemetry("/runtime-profile"))
    .length;
  await clickElement(
    `document.querySelector('[data-plan-mode=costs]')`,
    "SQL workspace control unavailable.",
  );
  await setInputValue("[data-sql-analysis-input=sql]", isolatedPlanSql);
  await sqlDraftRoundTripCoverage("standard_profile", "costs");
  await evaluate(
    `document.querySelector('[data-sql-analysis-input=sql]').scrollIntoView({ block: 'center', inline: 'nearest' })`,
  );
  await capture("topology-only-sql-input-1440x1024");
  await clickElement(
    `document.querySelector('[data-sql-analysis-action=view-plan]')`,
    "View plan unavailable.",
  );
  await waitFor(async () => {
    const routes = await routeTelemetry("/explain");
    return (
      routes.length === explainRoutesBefore + 1 &&
      routes.at(-1)?.method === "POST" &&
      routes.at(-1)?.status === 200 &&
      (await has("[data-sql-analysis-plan-result=success]"))
    );
  }, "Topology-only Explain response");
  assert(
    (await routeTelemetry("/runtime-profile")).length ===
      runtimeRoutesBeforeTopology,
    "Topology-only Explain executed Runtime Top.",
  );
  await clickButton("Open plan graph");
  const planRoot = ".explain-plan-overlay .execution-graph";
  await waitFor(() => has(planRoot), "Topology-only Explain plan graph");
  const topologyProjection = await projectionAfterRoute(
    "/explain",
    explainProjectionsBefore,
    "Topology-only Explain",
  );
  assert(
    topologyProjection.graph?.summary_state === null,
    "Explain plan graph must not carry a runtime execution summary.",
  );
  receipt.topology_only_graph = await assertDoctorGraph(
    topologyProjection,
    "Topology-only Explain",
    planRoot,
  );
  await capture("topology-only-plan-graph-1440x1024");
  if (project === "connected-sql-docker")
    await complexPlanNavigationCoverage(topologyProjection, planRoot);
  receipt.assertions.push("connected_sql_topology_only_plan_graph_visible");
  await clickElement(
    `Array.from(document.querySelectorAll('.explain-plan-overlay-header button')).find((button) => button.textContent.trim() === 'Close')`,
    "Explain plan graph close control was unavailable.",
  );
  await waitFor(
    () => has(".explain-plan-overlay").then((present) => !present),
    "Topology-only Explain close",
  );
  const runtimeRoutesBefore = (await routeTelemetry("/runtime-profile")).length;
  const runtimeSubmitsBefore = await evaluate(
    "window.__doctorE2eRuntimeSubmissions.length",
  );
  const runtimeProjectionsBefore = (
    await projectionTelemetry("/runtime-profile")
  ).length;
  await setInputValue("[data-sql-analysis-input=sql]", isolatedFixtureSql);
  await sqlDraftRoundTripCoverage("standard_profile", "costs");
  await evaluate(
    `document.querySelector('[data-sql-analysis-input=sql]').scrollIntoView({ block: 'center', inline: 'nearest' })`,
  );
  await capture("runtime-top-sql-input-1440x1024");
  receipt.assertions.push("connected_sql_runtime_top_input_evidence");
  await confirmStandardExecution(false);
  await evaluate(
    `(() => {    const button = document.querySelector('[data-sql-analysis-action=confirm-execution]:not(:disabled)');    if (!button) throw new Error('Runtime submit button unavailable');    button.click(); button.click();  })()`,
  );
  await waitFor(async () => {
    const routes = await routeTelemetry("/runtime-profile");
    return (
      routes.length === runtimeRoutesBefore + 1 &&
      routes.at(-1)?.method === "POST" &&
      routes.at(-1)?.status === 200 &&
      (await has(".workspace-doctor .doctor-summary"))
    );
  }, "one Runtime Top response");
  receipt.connected_runtime_profile_routes =
    await routeTelemetry("/runtime-profile");
  assert(
    receipt.connected_runtime_profile_routes.at(-1)
      ?.execution_attempt_id_valid === true,
    "Runtime submit did not bind a UUID execution attempt.",
  );
  assert(
    (await evaluate("window.__doctorE2eRuntimeSubmissions.length")) ===
      runtimeSubmitsBefore + 1,
    "Multiple Runtime requests started, including unfinished requests.",
  );
  receipt.assertions.push(
    "runtime_same_tick_double_submit_one_response_uuid_bound",
  );
  const runtimeProjection = await projectionAfterRoute(
    "/runtime-profile",
    runtimeProjectionsBefore,
    "Runtime Top",
  );
  assert(
    runtimeProjection.graph?.summary_state === "available" &&
      runtimeProjection.graph.hottest?.node_id,
    "Runtime Top report did not provide an available ExplainAnalyzer execution summary.",
  );
  receipt.runtime_top_graph = await assertDoctorGraph(
    runtimeProjection,
    "Runtime Top",
  );
  await capture("runtime-top-doctor-1440x1024");
  await runtimeTopNavigationCoverage(runtimeProjection);
  receipt.assertions.push(
    "connected_sql_runtime_top_summary_graph_and_backend_ranked_nodes",
  );
  if (project === "connected-sql-docker")
    await standardRuntimeOfficialDiscoveryLifecycleCoverage();
}
/** * It proves that a successful * standard Runtime Top Profile remains bound to its execution attempt after a * fresh Audit discovery, rather than accidentally becoming that discovery's * identity.  It never executes a second SQL statement. */ async function standardRuntimeOfficialDiscoveryLifecycleCoverage() {
  assert(
    officialProfileTransport === "real_loopback_api",
    "Standard Runtime Top official lifecycle requires the real loopback API, not the controlled official mock.",
  );
  await waitFor(
    () =>
      evaluate(
        `(() => {    const identity = window.__doctorE2eLastProfileFetchIdentity;    return identity?.identity?.kind === "verified_standard_attempt"      && identity.discovery_generation === null      && typeof identity.identity.execution_attempt_id_sha256 === "string";  })()`,
      ),
    "standard Runtime Top tagged Profile identity telemetry",
  );
  const standardIdentity = await evaluate(
    "window.__doctorE2eLastProfileFetchIdentity",
  );
  const runtimeRoutesBefore = (await routeTelemetry("/runtime-profile")).length;
  // Reuse the full native official-read coverage before deliberately changing
  // the active Audit discovery.  The helper validates request/response echo
  // against the current standard-attempt identity without recording raw IDs.
  await officialProfileCoverage();
  const initialOfficialRequestCount = (await officialRequestLog()).length;
  assert(
    initialOfficialRequestCount > 0,
    "Standard Runtime Top did not issue a native official read.",
  );
  const discoveryRoutesBefore = (await routeTelemetry(auditOverviewRoute))
    .length;
  const discoveryProjectionsBefore = (
    await projectionTelemetry(auditOverviewRoute)
  ).length;
  await openQueryMode("Audit history");
  await waitFor(
    () => has(".workspace-queries .audit-workload"),
    "Audit history for standard Runtime Top discovery refresh",
  );
  await waitFor(
    () => has("#official-profile-dialog").then((present) => !present),
    "official dialog unmounted before Queries discovery refresh",
  );
  await waitFor(async () => {
    const routes = await routeTelemetry(auditOverviewRoute);
    return (
      routes.length > discoveryRoutesBefore &&
      routes.at(-1)?.method === "POST" &&
      routes.at(-1)?.status === 200
    );
  }, "Queries Audit discovery refresh after standard Runtime Top");
  const discoveryProjection = await projectionAfterRoute(
    auditOverviewRoute,
    discoveryProjectionsBefore,
    "Queries Audit discovery refresh after standard Runtime Top",
  );
  assert(
    Number.isSafeInteger(discoveryProjection?.audit_generation) &&
      discoveryProjection.audit_generation > 0,
    "Queries discovery refresh did not publish a positive generation.",
  );
  const afterDiscoveryIdentity = await evaluate(
    "window.__doctorE2eLastProfileFetchIdentity",
  );
  assert(
    sameOfficialProfileIdentitySummary(
      standardIdentity.identity,
      afterDiscoveryIdentity?.identity,
    ) && afterDiscoveryIdentity?.discovery_generation === null,
    "Queries discovery replaced the standard Runtime Top Profile identity.",
  );
  assert(
    (await routeTelemetry("/runtime-profile")).length === runtimeRoutesBefore,
    "Queries discovery executed a second Runtime Top statement.",
  );
  const rereadRequestsBefore = (await officialRequestLog()).length;
  const rereadResponsesBefore = (await officialResponseLog()).length;
  await clickButton("Doctor");
  await waitFor(
    () => has(".workspace-doctor .official-profile-open"),
    "Doctor official entry after Queries discovery refresh",
  );
  await clickButton("Official reading");
  await waitFor(
    () =>
      has('#official-profile-dialog[aria-label="Official profile reading"]'),
    "standard Runtime Top official reread dialog",
  );
  assert(
    (await officialRequestLog()).length === rereadRequestsBefore,
    "Reopening Doctor issued an implicit official read.",
  );
  await clickButton("Read official analysis");
  await waitFor(async () => {
    const requests = await officialRequestLog();
    return (
      requests.length === rereadRequestsBefore + 1 &&
      requests.at(-1)?.status === 200
    );
  }, "standard Runtime Top official reread request");
  await waitFor(
    async () =>
      (await officialResponseLog()).length >= rereadResponsesBefore + 1 &&
      (await has(".official-profile-success")),
    "standard Runtime Top official reread response",
  );
  const rereadRequest = (await officialRequestLog()).at(-1);
  const rereadResponse = (await officialResponseLog()).at(-1);
  assertOfficialRequest(rereadRequest, [], standardIdentity);
  assertOfficialResponse(rereadResponse, [], standardIdentity);
  assertOfficialIdentityEcho(rereadRequest, rereadResponse);
  assert(
    (await routeTelemetry("/runtime-profile")).length === runtimeRoutesBefore,
    "Official reread after discovery executed another Runtime Top statement.",
  );
  await clickSelector(".official-profile-return");
  await waitFor(
    () => has("#official-profile-dialog").then((present) => !present),
    "official dialog closed before real session close",
  );
  // A real close removes the server session.  Doctor may retain a read-only
  // report view, but its connection-bound official identity must disappear and
  // must not make another request after the close.
  const officialRequestsBeforeClose = (await officialRequestLog()).length;
  await clickButton("Manage connection");
  await waitFor(
    () => has('[role="dialog"]'),
    "connection dialog before real session close",
  );
  await clickButton("Disconnect");
  await waitFor(
    () =>
      textOf('[role="dialog"] .connection-state').then((text) =>
        text.includes(
          "Connection closed; credentials were removed from the server session.",
        ),
      ),
    "real session close state",
  );
  await waitFor(
    () =>
      has(".workspace-doctor .official-profile-open").then(
        (present) => !present,
      ),
    "official entry cleared after real session close",
  );
  assert(
    (await officialRequestLog()).length === officialRequestsBeforeClose,
    "Closing the real session issued an official read.",
  );
  assert(
    (await routeTelemetry("/runtime-profile")).length === runtimeRoutesBefore,
    "Closing the real session executed Runtime Top.",
  );
  await clickButton("Close");
  await waitFor(
    () => has('[role="dialog"]').then((present) => !present),
    "closed connection dialog after standard Runtime Top lifecycle",
  );
  receipt.standard_runtime_official_lifecycle = {
    transport: "real_loopback_api",
    initial_identity_kind: standardIdentity.identity.kind,
    initial_discovery_generation: standardIdentity.discovery_generation,
    execution_attempt_id_sha256:
      standardIdentity.identity.execution_attempt_id_sha256,
    discovery_generation_after_queries: discoveryProjection.audit_generation,
    reread_identity_preserved: true,
    real_session_close_official_entry_cleared: true,
    runtime_request_count: runtimeRoutesBefore,
  };
  receipt.assertions.push(
    "standard_runtime_official_read_queries_discovery_reread_identity_preserved_real_session_close_clears_official_entry",
  );
}
async function complexPlanNavigationCoverage(projection, root) {
  const graph = projection.graph;
  assert(
    graph.node_count > 12 && graph.fragment_count >= 2,
    "Complex plan must exercise a multi-Fragment graph above the fit-all limit.",
  );
  const before = await doctorGraphDom(root);
  await clickButton("Full screen");
  await waitFor(
    () => has(".execution-graph.is-expanded"),
    "complex plan graph full screen",
  );
  const expanded = await doctorGraphDom(root);
  assert(
    expanded.card_count === before.card_count &&
      expanded.edge_count === before.edge_count,
    "Full screen changed the rendered DTO graph.",
  );
  await clickButton("Exit full screen");
  await waitFor(
    () => has(".execution-graph.is-expanded").then((present) => !present),
    "complex plan graph exit full screen",
  );
  await clickElement(
    `document.querySelector(${JSON.stringify(`${root} .react-flow__node`)})`,
    "Complex plan node could not be selected.",
  );
  await waitFor(
    async () => {
      const dom = await doctorGraphDom(root);
      return dom.selected_node_ids.length === 1 && dom.inspector_open;
    },
    "complex plan node inspector",
  );
  const inspector = await textOf(`${root} .node-inspector`);
  assert(
    inspector.includes("Plan estimates") || inspector.includes("Plan details"),
    "Explain node inspector did not render backend plan facts.",
  );
  receipt.complex_plan_navigation = {
    node_count: graph.node_count,
    fragment_count: graph.fragment_count,
    full_screen_round_trip: true,
    inspector_plan_facts: true,
  };
  receipt.assertions.push("complex_plan_full_screen_round_trip_and_node_inspector");
  await capture("complex-plan-inspector-1440x1024");
}
async function runtimeTopNavigationCoverage(projection) {
  const root = ".workspace-doctor .execution-graph";
  const hottest = projection.graph.hottest;
  const firstRow = ".workspace-doctor .plan-node-table tbody tr:first-child";
  assert(
    hottest.display && (await textOf(firstRow)).includes(hottest.display),
    "Plan-node table did not rank the backend hottest node first.",
  );
  await clickSelector(`${firstRow} .plan-node-link`);
  await waitFor(
    () =>
      evaluate(
        `(() => {    const root = document.querySelector(${JSON.stringify(root)});    const node = root?.querySelector('.plan-card.selected');    const canvas = root?.querySelector('.execution-graph-canvas');    if (!node || node.dataset.nodeId !== ${JSON.stringify(hottest.node_id)} || !root.querySelector('.node-inspector')) return false;    const n = node.getBoundingClientRect(), c = canvas.getBoundingClientRect();    return n.width > 0 && n.height > 0 && n.left >= Math.max(c.left, 0) && n.right <= Math.min(c.right, innerWidth)      && n.top >= Math.max(c.top, 0) && n.bottom <= Math.min(c.bottom, innerHeight);  })()`,
      ),
    "Runtime Top table row selects and reveals its graph node",
  );
  const operators = await evaluate(
    `document.querySelectorAll(${JSON.stringify(`${root} .node-inspector .inspector-operators tbody tr`)}).length`,
  );
  assert(operators > 0, "Runtime node inspector did not list its backend operators.");
  receipt.runtime_top_navigation = {
    state: "table_row_selects_visible_graph_node",
    node_id: hottest.node_id,
    inspector_operator_rows: operators,
  };
  receipt.assertions.push("runtime_top_table_row_node_selection_and_inspector");
}
async function narrowViewportEvidence() {
  await clickButton("Compare");
  await waitFor(
    () => has(".workspace-compare"),
    "Compare workspace for narrow evidence",
  );
  await cdp.send("Emulation.setDeviceMetricsOverride", {
    width: 900,
    height: 1024,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await wait(100);
  const dimensions = await evaluate(
    `({ viewport: { width: window.innerWidth, height: window.innerHeight }, scroll: { width: document.documentElement.scrollWidth, height: document.documentElement.scrollHeight } })`,
  );
  receipt.narrow_viewport = {
    ...dimensions,
    desktop_horizontal_scroll:
      dimensions.scroll.width > dimensions.viewport.width,
  };
  assert(
    dimensions.scroll.width <= dimensions.viewport.width,
    "The 900px Compare view overflowed the document horizontally.",
  );
  await capture("narrow-900x1024");
  receipt.assertions.push("narrow_compare_document_no_horizontal_overflow");
  await cdp.send("Emulation.setDeviceMetricsOverride", {
    width: 1440,
    height: 1024,
    deviceScaleFactor: 1,
    mobile: false,
  });
}
async function assertDoctorGraph(projection, description, root = ".workspace-doctor .execution-graph") {
  const graph = projection.graph;
  assert(graph && graph.node_count > 0, `${description} has no backend graph nodes.`);
  await waitFor(
    async () => (await doctorGraphDom(root)).layout_state === "ready",
    `${description} execution graph layout`,
  );
  // React Flow mounts edge paths a frame after the layout reports ready, so a
  // single sample can see 0 edges. Poll for the DTO counts, then assert on the
  // last observation so a real drift still fails with its actual numbers.
  try {
    await waitFor(async () => {
      const current = await doctorGraphDom(root);
      return current.card_count === graph.node_count && current.edge_count === graph.data_flow_pair_count;
    }, `${description} canvas reaches its DTO node/edge counts`);
  } catch {
    // Fall through to the explicit assertion below with the observed counts.
  }
  const dom = await doctorGraphDom(root);
  // The canvas draws one card per DTO node and one edge per proven directed
  // pair; unresolved relations stay listed in the legend instead of drawn.
  assert(
    dom.card_count === graph.node_count &&
      dom.edge_count === graph.data_flow_pair_count,
    `${description} canvas drifted from its DTO (${dom.card_count}/${graph.node_count} nodes, ${dom.edge_count}/${graph.data_flow_pair_count} edges).`,
  );
  // The Doctor data-flow view draws no Fragment frames; the raw-structure view keeps them.
  const expectedFrames = dom.view !== "flow" && graph.fragment_count >= 2 ? graph.fragment_count : 0;
  assert(
    dom.fragment_frame_count === expectedFrames && dom.fragment_frames_visible,
    `${description} Fragment frames drifted from the DTO fragment set.`,
  );
  if (graph.summary_state === "available") {
    assert(
      dom.table_rows === graph.summary_node_count &&
        dom.table_all_rows === graph.node_count &&
        dom.hot_most_count === graph.most_consuming_count &&
        dom.hot_second_count === graph.second_most_consuming_count,
      `${description} plan-node table or hotspot marks drifted from execution_summary.`,
    );
  }
  receipt.assertions.push(`doctor_graph_matches_dto:${description}`);
  return { backend: graph, dom };
}
async function doctorGraphDom(root = ".workspace-doctor .execution-graph") {
  return evaluate(
    `(() => {    const root = document.querySelector(${JSON.stringify(root)});    const canvas = root?.querySelector('.execution-graph-canvas');    const visible = (element) => { const rect = element.getBoundingClientRect(); return rect.width > 0 && rect.height > 0; };    const cards = Array.from(root?.querySelectorAll('.plan-card') ?? []);    return {      present: Boolean(root),      layout_state: canvas?.dataset.layoutState ?? null,      view: canvas?.dataset.view ?? null,      node_count: root?.querySelectorAll('.react-flow__node').length ?? 0,      card_count: cards.length,      visible_card_count: cards.filter(visible).length,      edge_count: root?.querySelectorAll('.react-flow__edge').length ?? 0,      fragment_frame_count: root?.querySelectorAll('.fragment-frame').length ?? 0,      fragment_frames_visible: Array.from(root?.querySelectorAll('.fragment-frame') ?? []).every(visible),      hot_most_count: root?.querySelectorAll('.plan-card.hot-most').length ?? 0,      hot_second_count: root?.querySelectorAll('.plan-card.hot-second').length ?? 0,      selected_node_ids: cards.filter((card) => card.classList.contains('selected')).map((card) => card.dataset.nodeId),      inspector_open: Boolean(root?.querySelector('.node-inspector')),      table_rows: document.querySelectorAll('.workspace-doctor .plan-node-table tbody tr:not(.unmeasured)').length,      table_all_rows: document.querySelectorAll('.workspace-doctor .plan-node-table tbody tr').length,      first_table_row_plan: document.querySelector('.workspace-doctor .plan-node-table tbody tr .plan-node-link small')?.textContent?.trim() ?? null,      first_table_row: document.querySelector('.workspace-doctor .plan-node-table tbody tr')?.textContent?.trim() ?? null,    };  })()`,
  );
}
async function openQueryMode(label) {
  if (!(await has(".workspace-queries"))) await clickButton("Queries");
  await waitFor(() => has(".workspace-queries .query-modes"), "Queries mode switch");
  await clickElement(
    `Array.from(document.querySelectorAll('.workspace-queries .query-modes [role="tab"]')).find((tab) => tab.textContent.trim() === ${JSON.stringify(label)})`,
    `Query mode ${label} is unavailable.`,
  );
  await waitFor(
    () => evaluate(`Array.from(document.querySelectorAll('.workspace-queries .query-modes [role="tab"]')).some((tab) => tab.textContent.trim() === ${JSON.stringify(label)} && tab.getAttribute('aria-selected') === 'true')`),
    `Query mode ${label} selected`,
  );
}
async function navigate(url) {
  await cdp.send("Page.navigate", { url });
  await waitFor(() => has(".app-shell"), "application shell");
}
async function installRouteTelemetry() {
  await evaluate(`(() => {    if (Array.isArray(window.__doctorE2eRouteTelemetry)) return;    const nativeFetch = window.fetch.bind(window);    const routes = [];    const projections = [];    const runtimeSubmissions = [];    const officialRequests = [];    const officialResponses = [];    const profileFetchErrors = [];    const summarizeGraph = (graph, summary) => {      if (!graph) return null;      const nodes = Array.isArray(graph.nodes) ? graph.nodes : [];      const edges = Array.isArray(graph.edges) ? graph.edges : [];      const summaryNodes = summary?.state === 'available' && Array.isArray(summary.nodes) ? summary.nodes : [];      const nodeIdSet = new Set(nodes.map((node) => node.node_id));      const fragmentByNode = new Map(summaryNodes.map((node) => [node.node_id, node.fragment_ref]));      const fragments = new Set(nodes.map((node) => fragmentByNode.get(node.node_id) ?? node.fragment_id).filter((fragment) => fragment !== null && fragment !== undefined));      let hottest = null;      for (const node of summaryNodes) if (Number.isInteger(node.time_share_basis_points) && (!hottest || node.time_share_basis_points > hottest.time_share_basis_points)) hottest = node;      return {        state: graph.state ?? null,        topology_source: graph.topology_source ?? null,        node_count: nodes.length,        edge_count: edges.length,        explicit_edge_count: edges.filter((edge) => edge?.source === 'explicit').length,        derived_edge_count: edges.filter((edge) => edge?.source === 'verified_derived').length,        data_flow_pair_count: new Set(edges.map((edge) => edge?.direction_semantics === 'parent_to_child' ? [edge.to, edge.from] : [edge?.from, edge?.to]).filter(([from, to]) => nodeIdSet.has(from) && nodeIdSet.has(to) && from !== to).map((pair) => JSON.stringify(pair))).size,        unresolved_relation_count: Array.isArray(graph.unresolved_relations) ? graph.unresolved_relations.length : 0,        fragment_count: fragments.size,        node_types: nodes.map((node) => node.plan_node_type ?? null),        summary_state: summary?.state ?? null,        summary_reasons: Array.isArray(summary?.reasons) ? summary.reasons : [],        summary_node_count: summaryNodes.length,        most_consuming_count: summaryNodes.filter((node) => node.hotspot === 'most_consuming').length,        second_most_consuming_count: summaryNodes.filter((node) => node.hotspot === 'second_most_consuming').length,        hottest: hottest ? { node_id: hottest.node_id, basis_points: hottest.time_share_basis_points, display: hottest.time_share_display ?? null, hotspot: hottest.hotspot } : null,      };    };    const captureProjection = async (pathname, body) => {      const snapshot = body?.snapshot;      const report = body?.outcome?.report;      const graphView = snapshot?.graph_view ?? report?.graph_view;      const executionSummary = report?.execution_summary ?? null;      if ((pathname.endsWith('/profiles/fetch') || pathname.endsWith('/runtime-profile')) && body?.state === 'fetched' && body?.query) {        window.__doctorE2eLastProfileFetchIdentity = await summarizeProfileFetchIdentity(body);        const query = body.query;        const nodes = report?.graph_view?.nodes;        window.__doctorE2eOfficialNodeCandidates = Array.isArray(nodes) ? nodes          .filter((node) => node?.entity_kind === 'plan_node' && typeof node?.plan_node_id === 'string' && /^\\d+$/.test(node.plan_node_id))          .map((node) => ({ node_id: node.node_id ?? null, plan_node_id: Number(node.plan_node_id) }))          .filter((node) => typeof node.node_id === 'string' && Number.isSafeInteger(node.plan_node_id) && node.plan_node_id >= 0 && node.plan_node_id <= 2147483647) : [];      }      const top = pathname.endsWith('/audit/overview') ? body?.primary?.top : pathname.endsWith('/audit/occurrences') ? body?.top : null;      const auditTop = top ? {        full_window_count: typeof top.full_window_count === 'string' ? top.full_window_count : null,        requested_limit: Number.isInteger(top.requested_limit) ? top.requested_limit : null,        returned_count: Number.isInteger(top.returned_count) ? top.returned_count : null,        truncated: typeof top.truncated === 'boolean' ? top.truncated : null,      } : null;      projections.push({        path: pathname,
// Persist only fixed-window width, enums and presence flags, never
// user/database/digest values, Query IDs, SQL or connection secrets.
profile_fetch_state: pathname.endsWith('/profiles/fetch') ? body?.state ?? null : null,        audit_generation: Number.isInteger(body?.discovery_generation) ? body.discovery_generation : null,        audit_scope: body?.scope ? {          window_seconds: body.scope.window.end_seconds - body.scope.window.start_seconds,          state: body.scope.state ?? null,          has_user_filter: body.scope.user != null,          has_database_filter: body.scope.database != null,          has_digest_filter: body.scope.native_digest != null,        } : null,        audit_primary_state: pathname.endsWith('/audit/overview') ? body?.primary?.state ?? null : null,        audit_top: auditTop,        audit_coverage: body?.coverage ? { retained_event_count: body.coverage.retained_event_count ?? null, retained_family_count: body.coverage.retained_family_count ?? null, raw_row_count: body.coverage.raw_row_count ?? null, missing_digest_count: body.coverage.missing_digest_count ?? null } : null,        graph: summarizeGraph(graphView, executionSummary),        family_detail: pathname.endsWith('/audit/family-detail') ? {          membership_state: body?.family?.state ?? null,          distribution_state: body?.distribution?.state ?? null,          bucket_count: Array.isArray(body?.distribution?.buckets) ? body.distribution.buckets.length : 0,          representative_states: ['recent', 'slowest', 'profile_known'].map((role) => body?.representatives?.[role]?.state ?? null),
// Presence only: no Query IDs, native digest, SQL or representative payload.
        raw_sql_present: 'sql' in (body ?? {}),          raw_profile_present: 'profile' in (body ?? {}),        } : null,        });    };    const officialPath = (rawUrl) => new URL(rawUrl, window.location.href).pathname;    const officialConnectionId = (rawPath) => {      const match = /^\\/api\\/v1\\/connections\\/([^/]+)\\/profiles\\/official-analysis$/.exec(String(rawPath));      if (!match) return null;      try { return decodeURIComponent(match[1]); } catch { return null; }    };    const jsonBody = (init) => {      if (typeof init?.body !== 'string') return null;      try { return JSON.parse(init.body); } catch { return null; }    };    const normalizedRoutePath = (rawUrl) => officialPath(rawUrl)      .replace(new RegExp("^/api/v1/connections/[^/]+"), "/api/v1/connections/{connection_id}");    const recordControlledRoute = (input, init, control) => {      const request = input instanceof Request ? input : null;      const rawUrl = request ? request.url : String(input);      const pathname = normalizedRoutePath(rawUrl);      const method = (init?.method ?? request?.method ?? "GET").toUpperCase();      const runtime = control?.runtime === true && pathname.endsWith("/runtime-profile");      let executionAttemptIdValid = null;      if (runtime && typeof init?.body === "string") {        try { executionAttemptIdValid = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(JSON.parse(init.body).execution_attempt_id ?? ""); }        catch { executionAttemptIdValid = false; }      }      if (runtime) runtimeSubmissions.push(executionAttemptIdValid);      routes.push({ path: pathname, method, execution_attempt_id_valid: executionAttemptIdValid, status: control?.status === 0 ? 0 : 504, duration_ms: 0 });    };    const hashOpaque = async (value) => {      if (typeof value !== 'string' || !value) return null;      const bytes = new TextEncoder().encode(value);      return Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', bytes)), (byte) => byte.toString(16).padStart(2, '0')).join('');    };    const record = (value) => typeof value === 'object' && value !== null && !Array.isArray(value);    const exactKeys = (value, keys) => record(value) && Object.keys(value).length === keys.length && keys.every((key) => Object.hasOwn(value, key));    const positiveGeneration = (value) => Number.isSafeInteger(value) && value > 0;    const nodeIds = (value) => Array.isArray(value) && value.every((id) => Number.isSafeInteger(id) && id >= 0 && id <= 2147483647);    const summarizeIdentity = async (value) => {      if (exactKeys(value, ['kind', 'discovery_generation']) && value.kind === 'discovery' && positiveGeneration(value.discovery_generation)) {        return { kind: 'discovery', discovery_generation: value.discovery_generation, execution_attempt_id_sha256: null };      }      if (exactKeys(value, ['kind', 'execution_attempt_id']) && value.kind === 'verified_standard_attempt' && typeof value.execution_attempt_id === 'string' && value.execution_attempt_id) {        const execution_attempt_id_sha256 = await hashOpaque(value.execution_attempt_id);        return execution_attempt_id_sha256 ? { kind: 'verified_standard_attempt', discovery_generation: null, execution_attempt_id_sha256 } : null;      }      return null;    };    const summarizeProfileFetchIdentity = async (body) => {      const query = body?.query;      if (!record(query) || typeof query.connection_id !== 'string' || !query.connection_id || typeof query.query_id !== 'string' || !query.query_id) return null;      const eligibilityPresent = Object.hasOwn(body ?? {}, 'official_profile_eligibility');      const identity = eligibilityPresent        ? exactKeys(body.official_profile_eligibility, ['execution_attempt_id', 'method']) && body.official_profile_eligibility.method === 'standard_profile' && query.discovery_generation === null          ? await summarizeIdentity({ kind: 'verified_standard_attempt', execution_attempt_id: body.official_profile_eligibility.execution_attempt_id })          : null        : await summarizeIdentity({ kind: 'discovery', discovery_generation: query.discovery_generation });      if (!identity) return null;      const [connection_id_sha256, query_id_sha256] = await Promise.all([hashOpaque(query.connection_id), hashOpaque(query.query_id)]);      return connection_id_sha256 && query_id_sha256 ? { connection_id_sha256, query_id_sha256, discovery_generation: query.discovery_generation, identity } : null;    };    const summarizeOfficialRequest = async (rawPath, method, body) => {      const connectionId = officialConnectionId(rawPath);      if (!connectionId || !exactKeys(body, ['connection_id', 'query_id', 'identity', 'node_ids']) || typeof body.connection_id !== 'string' || !body.connection_id || typeof body.query_id !== 'string' || !body.query_id || !nodeIds(body.node_ids)) return null;      const identity = await summarizeIdentity(body.identity);      const [path_connection_id_sha256, connection_id_sha256, query_id_sha256] = await Promise.all([hashOpaque(connectionId), hashOpaque(body.connection_id), hashOpaque(body.query_id)]);      return identity && path_connection_id_sha256 && connection_id_sha256 && query_id_sha256        ? { endpoint: 'official_analysis', path_connection_id_sha256, body: { connection_id_sha256, query_id_sha256, identity, node_ids: [...body.node_ids] } }        : null;    };    const summarizeOfficialResponse = async (body) => {      const query = body?.query;      if (!record(body) || !record(query) || !nodeIds(body.node_ids) || typeof query.connection_id !== 'string' || !query.connection_id || typeof query.query_id !== 'string' || !query.query_id) return null;      const identity = await summarizeIdentity(body.identity);      const generationMatches = identity?.kind === 'discovery'        ? query.discovery_generation === identity.discovery_generation        : identity?.kind === 'verified_standard_attempt' && query.discovery_generation === null;      const [connection_id_sha256, query_id_sha256] = await Promise.all([hashOpaque(query.connection_id), hashOpaque(query.query_id)]);      return identity && generationMatches && connection_id_sha256 && query_id_sha256        ? { kind: body.kind ?? null, connection_id_sha256, query_id_sha256, discovery_generation: query.discovery_generation, identity, node_ids: [...body.node_ids], display_hash: typeof body.display_hash === 'string' ? body.display_hash : null, source_hash: typeof body.source_hash === 'string' ? body.source_hash : null, display_text_present: typeof body.display_text === 'string', text_present: typeof body.text === 'string', raw_display_distinct: typeof body.text === 'string' && typeof body.display_text === 'string' && body.text !== body.display_text }        : null;    };    Object.defineProperty(window, '__doctorE2eOfficialConnectionId', { configurable: true, value: officialConnectionId });    Object.defineProperty(window, '__doctorE2eOfficialRequestSummary', { configurable: true, value: summarizeOfficialRequest });    Object.defineProperty(window, '__doctorE2eOfficialResponseSummary', { configurable: true, value: summarizeOfficialResponse });    window.fetch = async (input, init) => {      const request = input instanceof Request ? input : null;      const rawUrl = request ? request.url : String(input);      const rawPath = officialPath(rawUrl);      const pathname = rawPath        .replace(/^\\/api\\/v1\\/connections\\/[^/]+/, "/api/v1/connections/{connection_id}");      const method = (init?.method ?? request?.method ?? "GET").toUpperCase();      const officialConnection = officialConnectionId(rawPath);      const official = officialConnection !== null;      const bypassOfficialLedger = init?.__doctorE2eOfficialMockBypass === true;      const officialBody = jsonBody(init);      const signal = init?.signal ?? request?.signal ?? null;      const officialRequestSummary = official && !bypassOfficialLedger        ? await summarizeOfficialRequest(rawPath, method, officialBody)        : null;      const officialRequest = official && !bypassOfficialLedger ? {        ...(officialRequestSummary ?? { endpoint: 'invalid_official_analysis', path_connection_id_sha256: null, body: null }),        method,        status: null,        aborted: Boolean(signal?.aborted),      } : null;      if (officialRequest) {        officialRequests.push(officialRequest);        signal?.addEventListener('abort', () => { officialRequest.aborted = true; }, { once: true });      }      const executionRoute = pathname.endsWith('/runtime-profile');      let executionAttemptIdValid = null;      if (executionRoute && typeof init?.body === 'string') {        try {          const body = JSON.parse(init.body);          executionAttemptIdValid = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(body.execution_attempt_id ?? '');          } catch {          executionAttemptIdValid = false;          }      }      if (pathname.endsWith('/runtime-profile')) runtimeSubmissions.push(executionAttemptIdValid);      const started = performance.now();      try {        const response = await nativeFetch(input, init);        const routeRecord = { path: pathname, method, execution_attempt_id_valid: executionAttemptIdValid, status: response.status, duration_ms: Math.round(performance.now() - started), failure_kind: null };        routes.push(routeRecord);        if (!response.ok) void response.clone().json().then((body) => {          const error = body?.error ?? body;          const known = ['connection_expired', 'connection_failed', 'remote_timeout', 'authentication_failed', 'tls_failed', 'permission_denied', 'query_discovery_stale', 'capability_unavailable'];          routeRecord.failure_kind = known.includes(error?.message_id) ? error.message_id : known.includes(error?.code) ? error.code : 'other_typed_error';        }).catch(() => { routeRecord.failure_kind = 'unreadable_error'; });        if (official && !bypassOfficialLedger) {          if (officialRequest) officialRequest.status = response.status;          void response.clone().json()            .then((body) => summarizeOfficialResponse(body))            .then((summary) => { if (summary) officialResponses.push(summary); })            .catch(() => undefined);        }        if (pathname.endsWith('/profiles/fetch') && !response.ok) {          void response.clone().json().then((body) => {            const error = body?.error ?? body;            profileFetchErrors.push({              status: response.status,              code: typeof error?.code === 'string' ? error.code : null,              message_id: typeof error?.message_id === 'string' ? error.message_id : null,              recoverability: typeof error?.recoverability === 'string' ? error.recoverability : null,            });          }).catch(() => profileFetchErrors.push({ status: response.status, code: null, message_id: null, recoverability: null }));        }        if (response.ok && (pathname.endsWith('/analyses') || pathname.endsWith('/explain') || pathname.endsWith('/runtime-profile') || pathname.endsWith('/audit/overview') || pathname.endsWith('/audit/occurrences') || pathname.endsWith('/audit/families') || pathname.endsWith('/audit/family-detail') || pathname.endsWith('/profiles/fetch'))) {          void response.clone().json().then((body) => captureProjection(pathname, body)).catch(() => undefined);        }        return response;      } catch (error) {        if (officialRequest) officialRequest.status = 0;        routes.push({ path: pathname, method, execution_attempt_id_valid: executionAttemptIdValid, status: 0, duration_ms: Math.round(performance.now() - started) });        throw error;      }    };    Object.defineProperty(window, "__doctorE2eRouteTelemetry", { configurable: true, value: routes });    Object.defineProperty(window, "__doctorE2eRuntimeSubmissions", { configurable: true, value: runtimeSubmissions });    Object.defineProperty(window, "__doctorE2eProjectionTelemetry", { configurable: true, value: projections });    Object.defineProperty(window, "__doctorE2eOfficialRequests", { configurable: true, value: officialRequests });    Object.defineProperty(window, "__doctorE2eOfficialResponses", { configurable: true, value: officialResponses });    Object.defineProperty(window, "__doctorE2eProfileFetchErrors", { configurable: true, value: profileFetchErrors });    Object.defineProperty(window, "__doctorE2eRecordControlledRoute", { configurable: true, value: recordControlledRoute });    Object.defineProperty(window, "__doctorE2eLastProfileFetchIdentity", { configurable: true, writable: true, value: null });    Object.defineProperty(window, "__doctorE2eOfficialNodeCandidates", { configurable: true, writable: true, value: [] });  })()`);
}
async function routeTelemetry(routeSuffix) {
  return evaluate(
    `(window.__doctorE2eRouteTelemetry ?? []).filter((route) => route.path.endsWith(${JSON.stringify(routeSuffix)})).map(({ path, method, status, duration_ms, execution_attempt_id_valid, candidate_sql_matches_isolated_fixture, failure_kind }) => ({ path, method, status, duration_ms, execution_attempt_id_valid, candidate_sql_matches_isolated_fixture: candidate_sql_matches_isolated_fixture ?? null, failure_kind: failure_kind ?? null }))`,
  );
}
async function projectionTelemetry(routeSuffix) {
  return evaluate(
    `(window.__doctorE2eProjectionTelemetry ?? []).filter((projection) => projection.path.endsWith(${JSON.stringify(routeSuffix)})).map((projection) => ({ ...projection }))`,
  );
}
async function profileFetchErrors() {
  return evaluate(
    "(window.__doctorE2eProfileFetchErrors ?? []).map((error) => ({ ...error }))",
  );
}
async function projectionAfterRoute(routeSuffix, priorCount, description) {
  await waitFor(
    async () => (await projectionTelemetry(routeSuffix)).length > priorCount,
    `${description} DTO projection telemetry`,
  );
  const projections = await projectionTelemetry(routeSuffix);
  return projections.at(-1);
}
async function redactVisibleIdentifiersForEvidence({ redactSql = false } = {}) {
  await evaluate(
    `window.__doctorE2eRedactSqlForCapture = ${redactSql ? "true" : "false"}`,
  );
  const redaction = await evaluate(
    `(() => {    const uuid = /\\b[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}\\b/gi;    const aliases = window.__doctorE2eScreenshotAliases ??= new Map();    const connectionAliases = window.__doctorE2eScreenshotConnectionAliases ??= new Map();    const changed = window.__doctorE2eScreenshotText = [];    const aliasFor = (value) => {      const key = value.toLowerCase();      if (!aliases.has(key)) aliases.set(key, 'QID-' + String(aliases.size + 1).padStart(3, '0'));      return aliases.get(key);    };    const connectionAliasFor = (value) => {      const key = value.toLowerCase();      if (!connectionAliases.has(key)) connectionAliases.set(key, 'CID-' + String(connectionAliases.size + 1).padStart(3, '0'));      return connectionAliases.get(key);    };    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);    const nodes = [];    while (walker.nextNode()) nodes.push(walker.currentNode);    for (const node of nodes) {      if (node.parentElement?.closest('script,style')) continue;      const original = node.nodeValue ?? '';      const replacement = original.replace(uuid, aliasFor);      if (original !== replacement) {        changed.push({ node, original, replacement });        node.nodeValue = replacement;      }    }    const connectionIdLabels = new Set(['Connection ID', '连接 ID', '连接ID']);    const connectionIdPattern = /^[0-9a-f]{32}(?:[0-9a-f]{16})?$/i;    const connectionValueElements = [      ...Array.from(document.querySelectorAll('dt')).filter(label => connectionIdLabels.has(label.textContent.trim())).map(label => label.nextElementSibling),      ...document.querySelectorAll('.running-profile-connection code, .sql-analysis-connection code, .sql-analysis-locked code, .sql-analysis-confirm-action code, section[aria-label="Connected session"] code, section[aria-label="已连接会话"] code'),    ].filter(Boolean);    for (const valueElement of connectionValueElements) {      const connectionId = valueElement.textContent?.trim() ?? '';      if (!connectionIdPattern.test(connectionId)) continue;      const replacementValue = connectionAliasFor(connectionId);      const valueWalker = document.createTreeWalker(valueElement, NodeFilter.SHOW_TEXT);      const valueNodes = [];      while (valueWalker.nextNode()) valueNodes.push(valueWalker.currentNode);      for (const node of valueNodes) {        const original = node.nodeValue ?? '';        const replacement = original.split(connectionId).join(replacementValue);        if (original !== replacement) {          changed.push({ node, original, replacement });          node.nodeValue = replacement;        }      }    }    // Audit native digests share the 32-hex shape with session IDs. Redact remaining opaque hex identities too; keep the global leak guard.
    const opaqueAliases = window.__doctorE2eScreenshotOpaqueAliases ??= new Map();    for (const node of nodes) {      if (node.parentElement?.closest('script,style')) continue;      const original = node.nodeValue ?? '';      const replacement = original.replace(/\\b[0-9a-f]{32}(?:[0-9a-f]{16})?\\b/gi, value => {        const key = value.toLowerCase();        if (connectionAliases.has(key)) return connectionAliases.get(key);        if (!opaqueAliases.has(key)) opaqueAliases.set(key, 'OPAQUE-' + String(opaqueAliases.size + 1).padStart(3, '0'));        return opaqueAliases.get(key);      });      if (original !== replacement) { changed.push({ node, original, replacement }); node.nodeValue = replacement; }    }    const redactSql = window.__doctorE2eRedactSqlForCapture === true;    let sqlInputCount = 0;    let nativeOfficialTextCount = 0;    if (redactSql) {      for (const input of document.querySelectorAll('textarea[data-sql-analysis-input="sql"]')) {        const original = input.value;        if (!original) continue;        const replacement = '[SQL redacted for development evidence]';        changed.push({ node: input, original, replacement, kind: 'value' });        input.value = replacement;        sqlInputCount += 1;      }      for (const container of document.querySelectorAll('pre.sql-analysis-official-text, .running-profile-entry pre.official-profile-display-text')) {        const textWalker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);        const textNodes = [];        while (textWalker.nextNode()) textNodes.push(textWalker.currentNode);        for (const node of textNodes) {          const original = node.nodeValue ?? '';          if (!original) continue;          const replacement = '[Native official text redacted for development evidence]';          changed.push({ node, original, replacement });          node.nodeValue = replacement;          nativeOfficialTextCount += 1;        }      }    }    const visibleText = document.body.innerText;    const visibleConnectionIds = /\\b[0-9a-f]{32}(?:[0-9a-f]{16})?\\b/i.test(visibleText);    return {      aliases: aliases.size,      connection_aliases: connectionAliases.size,      sql_inputs_redacted: sqlInputCount,      native_official_text_nodes_redacted: nativeOfficialTextCount,      full_uuid_visible: /\\b[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}\\b/i.test(visibleText),      full_connection_id_visible: visibleConnectionIds,    };  })()`,
  );
  assert(
    !redaction.full_uuid_visible,
    "A full Query UUID remained visible before screenshot capture.",
  );
  assert(
    !redaction.full_connection_id_visible,
    "A full Connection ID remained visible before screenshot capture.",
  );
  receipt.screenshot_redaction = {
    strategy: "stable_query_and_connection_alias",
    alias_count: redaction.aliases,
    connection_alias_count: redaction.connection_aliases,
    full_uuid_visible: false,
    full_connection_id_visible: false,
  };
  if (!receipt.assertions.includes("screenshots_exclude_full_query_uuid"))
    receipt.assertions.push("screenshots_exclude_full_query_uuid");
  if (!receipt.assertions.includes("screenshots_exclude_visible_connection_id"))
    receipt.assertions.push("screenshots_exclude_visible_connection_id");
}
async function capture(name, { redactSql = false } = {}) {
  // A completed request or tab switch can introduce fresh identifiers after the
  // previous capture. Redact immediately before every evidence screenshot.
  try {
    await redactVisibleIdentifiersForEvidence({ redactSql });
    const fullUuidVisible = await evaluate(
      `/\\b[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}\\b/i.test(document.body.innerText)`,
    );
    assert(
      !fullUuidVisible,
      `Screenshot ${name} would contain a full Query UUID.`,
    );
    const fullConnectionIdVisible = await evaluate(
      `/\\b[0-9a-f]{32}(?:[0-9a-f]{16})?\\b/i.test(document.body.innerText)`,
    );
    assert(
      !fullConnectionIdVisible,
      `Screenshot ${name} would contain a full Connection ID.`,
    );
    const image = await cdp.send("Page.captureScreenshot", {
      format: "png",
      captureBeyondViewport: false,
      fromSurface: true,
    });
    const target = join(runRoot, `${name}.png`);
    writeFileSync(target, Buffer.from(image.data, "base64"));
    receipt.screenshots.push({
      relative_path: `${name}.png`,
      byte_length: statSync(target).size,
      sha256: createHash("sha256").update(readFileSync(target)).digest("hex"),
    });
  } finally {
    // Screenshot-only presentation must not leak into subsequent UI assertions.
    await evaluate(
      `(() => {      for (const { node, original, replacement, kind } of [...(window.__doctorE2eScreenshotText ?? [])].reverse()) {        if (kind === 'value') {          if (node.isConnected && node.value === replacement) node.value = original;        } else if (node.isConnected && node.nodeValue === replacement) {          node.nodeValue = original;        }      }      window.__doctorE2eScreenshotText = [];      delete window.__doctorE2eRedactSqlForCapture;    })()`,
    );
  }
}
async function clickButton(text) {
  // While a modal dialog is open only its own buttons are reachable, as for a
  // user: a same-named button behind it (e.g. a graph node's "Close") must
  // not be picked first just because it comes earlier in the document.
  // The button is found again by its text on every attempt: a position in the
  // document goes stale while the page re-renders (after a multi-BE connect
  // the background keeps adding buttons), and would then name another one.
  // A button disabled by an in-flight read becomes clickable when it settles;
  // clickElement waits a bounded 10 s for that.
  const find = `(() => { const dialog = document.querySelector('[role="dialog"][aria-modal="true"]'); return Array.from(document.querySelectorAll('button')).find((item) => (!dialog || dialog.contains(item)) && (item.textContent.trim() === ${JSON.stringify(text)} || item.getAttribute('aria-label') === ${JSON.stringify(text)}) && !item.disabled) ?? null; })()`;
  await clickElement(find, `Button was unavailable: ${text}`);
}
async function clickSelector(selector) {
  await clickElement(
    `document.querySelector(${JSON.stringify(selector)})`,
    `Element was unavailable: ${selector}`,
  );
}
async function clickReactFlowPaneBackground(selector) {
  const point = await evaluate(
    `(() => {    const pane = document.querySelector(${JSON.stringify(selector)});    if (!pane) return null;    const rect = pane.getBoundingClientRect();    const left = Math.max(0, rect.left + 12), right = Math.min(innerWidth, rect.right - 12);    const top = Math.max(0, rect.top + 12), bottom = Math.min(innerHeight, rect.bottom - 12);    if (right <= left || bottom <= top) return null;    for (const yRatio of [0.16, 0.34, 0.58, 0.82]) for (const xRatio of [0.12, 0.28, 0.5, 0.72, 0.88]) {      const x = left + (right - left) * xRatio, y = top + (bottom - top) * yRatio;      const hit = document.elementFromPoint(x, y);      if (hit === pane) return { x, y };    }    return null;  })()`,
  );
  assert(point, `React Flow pane has no visible blank background: ${selector}`);
  await cdp.send("Input.dispatchMouseEvent", {
    type: "mousePressed",
    x: point.x,
    y: point.y,
    button: "left",
    buttons: 1,
    clickCount: 1,
  });
  await cdp.send("Input.dispatchMouseEvent", {
    type: "mouseReleased",
    x: point.x,
    y: point.y,
    button: "left",
    buttons: 0,
    clickCount: 1,
  });
  await wait(30);
}
async function clickElement(
  elementExpression,
  unavailableMessage = "Element was unavailable.",
) {
  // Like clickButton: an element disabled by an in-flight read becomes
  // available when it settles; wait a bounded 10 s before failing.
  const locate = `(() => { const item = ${elementExpression}; if (!item || item.disabled) return null; item.scrollIntoView({ block: 'center', inline: 'center' }); const rect = item.getBoundingClientRect(); if (rect.width <= 0 || rect.height <= 0) return null; const x = rect.left + rect.width / 2, y = rect.top + rect.height / 2; const hit = document.elementFromPoint(x, y); return hit && item.contains(hit) ? { x, y } : null; })()`;
  const deadline = Date.now() + 10_000;
  let point = await evaluate(locate);
  while (!point && Date.now() < deadline) {
    await wait(100);
    point = await evaluate(locate);
  }
  if (!point) {
    // Name what covers the element, so a failure says why it was unreachable.
    const covered = await evaluate(`(() => { const item = ${elementExpression}; if (!item) return 'not found'; if (item.disabled) return 'disabled'; const rect = item.getBoundingClientRect(); if (rect.width <= 0 || rect.height <= 0) return 'not rendered'; const hit = document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2); return hit ? 'covered by ' + hit.tagName.toLowerCase() + (hit.className && typeof hit.className === 'string' ? '.' + hit.className.trim().split(/\\s+/).join('.') : '') : 'outside the viewport'; })()`);
    assert(false, `${unavailableMessage} (${covered})`);
  }
  await cdp.send("Input.dispatchMouseEvent", {
    type: "mousePressed",
    x: point.x,
    y: point.y,
    button: "left",
    buttons: 1,
    clickCount: 1,
  });
  await cdp.send("Input.dispatchMouseEvent", {
    type: "mouseReleased",
    x: point.x,
    y: point.y,
    button: "left",
    buttons: 0,
    clickCount: 1,
  });
  await wait(30);
}
async function setInputValue(selector, value) {
  const changed = await evaluate(
    `(() => { const element = document.querySelector(${JSON.stringify(selector)}); if (!element || element.disabled) return false; const prototype = element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : element instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype; const setter = Object.getOwnPropertyDescriptor(prototype, 'value')?.set; setter?.call(element, ${JSON.stringify(value)}); element.dispatchEvent(new Event('input', { bubbles: true })); element.dispatchEvent(new Event('change', { bubbles: true })); return true; })()`,
  );
  assert(changed, `Input was unavailable: ${selector}`);
}
async function setInputAt(selector, index, value) {
  const changed = await evaluate(
    `(() => {    const element = Array.from(document.querySelectorAll(${JSON.stringify(selector)}))[${index}];    if (!element) return false;    const setter = Object.getOwnPropertyDescriptor(element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype, 'value')?.set;    setter?.call(element, ${JSON.stringify(value)});    element.dispatchEvent(new Event('input', { bubbles: true }));    element.dispatchEvent(new Event('change', { bubbles: true }));    return true;  })()`,
  );
  assert(changed, `Input was unavailable: ${selector}[${index}]`);
}
async function tabUntil(predicate, description) {
  for (let index = 0; index < 80; index += 1) {
    await key("Tab");
    const active = await activeSummary();
    receipt.keyboard_trace.push({ index, ...active });
    if (predicate(active)) return active;
  }
  fail(`Keyboard Tab could not reach ${description}.`);
}
async function key(keyName) {
  const code = keyName === "Enter" ? "Enter" : "Tab";
  await cdp.send("Input.dispatchKeyEvent", {
    type: "keyDown",
    key: keyName,
    code,
    text: keyName === "Enter" ? "\r" : undefined,
    unmodifiedText: keyName === "Enter" ? "\r" : undefined,
    windowsVirtualKeyCode: keyName === "Enter" ? 13 : 9,
    nativeVirtualKeyCode: keyName === "Enter" ? 13 : 9,
  });
  await cdp.send("Input.dispatchKeyEvent", {
    type: "keyUp",
    key: keyName,
    code,
    windowsVirtualKeyCode: keyName === "Enter" ? 13 : 9,
    nativeVirtualKeyCode: keyName === "Enter" ? 13 : 9,
  });
  await wait(30);
}
async function activeSummary() {
  return evaluate(
    `(() => { const element = document.activeElement; return element ? { tag: element.tagName, text: element.textContent.trim(), ariaLabel: element.getAttribute('aria-label'), value: element.value ?? null } : null; })()`,
  );
}
async function has(selector) {
  return Boolean(
    await evaluate(
      `Boolean(document.querySelector(${JSON.stringify(selector)}))`,
    ),
  );
}
async function textOf(selector) {
  return evaluate(
    `document.querySelector(${JSON.stringify(selector)})?.textContent ?? ''`,
  );
}
async function selectedProfileAvailability() {
  return evaluate(
    `Array.from(document.querySelectorAll('.selected-query dt')).find((item) => item.textContent.trim() === 'Availability')?.nextElementSibling?.textContent?.trim() ?? null`,
  );
}
async function evaluate(expression) {
  const result = await cdp.send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
  });
  if (result.exceptionDetails) {
    const detail = result.exceptionDetails;
    const exception =
      detail.exception?.description ?? detail.exception?.value ?? detail.text;
    const location =
      Number.isInteger(detail.lineNumber) &&
      Number.isInteger(detail.columnNumber)
        ? ` at ${detail.lineNumber}:${detail.columnNumber}`
        : "";
    fail(`Browser evaluation failed: ${exception}${location}`);
  }
  return result.result.value;
}
async function waitFor(check, description) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (await check()) return;
    await wait(100);
  }
  fail(`Timed out waiting for ${description}.`);
}
function assert(condition, message) {
  if (!condition) fail(message);
}
function fail(message) {
  throw new Error(message);
}
function wait(milliseconds) {
  return new Promise((resolvePromise) =>
    setTimeout(resolvePromise, milliseconds),
  );
}
function option(name) {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : undefined;
}
function requiredEnv(name) {
  const value = process.env[name];
  if (!value) fail(`Set ${name} for connected browser E2E.`);
  return value;
}
function stamp() {
  return new Date().toISOString().replace(/[:.]/g, "-");
}
function hashOpaque(value) {
  return createHash("sha256").update(String(value), "utf8").digest("hex");
}
function hashOpaqueIds(values) {
  return Array.isArray(values) && values.length
    ? hashOpaque([...values].sort().join("\0"))
    : null;
}
function parseSha256Allowlist(value) {
  const entries = String(value ?? "").trim()
    ? String(value)
        .trim()
        .split(/[\s,]+/)
        .filter(Boolean)
        .map((entry) => entry.toLowerCase())
    : [];
  if (entries.some((entry) => !/^[0-9a-f]{64}$/.test(entry)))
    fail("E2E_RETAINED_PROFILE_HASHES must contain only SHA-256 hashes.");
  return Object.freeze([...new Set(entries)]);
}
function resolveAuditFixtureWindow(value, currentProject, transport) {
  const normalized = String(value ?? "").trim();
  if (!normalized) return null;
  if (!expectedAuditWindowPresets.includes(normalized))
    fail(
      "E2E_AUDIT_FIXTURE_WINDOW must be one of " +
        expectedAuditWindowPresets.join(", ") +
        ".",
    );
  if (
    currentProject !== "connected-audit-docker" ||
    transport !== "controlled_mock_over_real_session"
  ) {
    fail(
      "E2E_AUDIT_FIXTURE_WINDOW is reserved for the fixed controlled B browser validation.",
    );
  }
  return normalized;
}
function resolveRuntimeUnknownGuardTransport(value, currentProject) {
  const normalized = String(value ?? "").trim();
  if (!normalized) return "real_loopback_api";
  if (normalized !== "controlled_mock_over_real_session") {
    fail(
      "E2E_RUNTIME_UNKNOWN_GUARD_TRANSPORT must be controlled_mock_over_real_session when set.",
    );
  }
  if (currentProject !== "connected-sql-docker") {
    fail(
      "E2E_RUNTIME_UNKNOWN_GUARD_TRANSPORT is reserved for the connected-sql-docker Task49 browser guard.",
    );
  }
  return normalized;
}
function deriveFixturePassword(secret, role) {
  return `sr3514-${role}-${secret}`;
}
function loadBuildIdentity() {
  const path = process.env.E2E_BUILD_IDENTITY_PATH;
  if (!path) return { verification: "not_supplied" };
  try {
    const identity = JSON.parse(readFileSync(path, "utf8"));
    if (
      identity?.schema_version !== "e2e-build-identity/v1" ||
      !/^[0-9a-f]{40}$/.test(identity.git_head ?? "") ||
      !/^[0-9a-f]{64}$/.test(identity.expected?.api?.sha256 ?? "") ||
      !/^[0-9a-f]{64}$/.test(identity.expected?.frontend?.tree_sha256 ?? "")
    ) {
      fail("E2E build identity is malformed.");
    }
    return identity;
  } catch (error) {
    fail(
      `E2E build identity is unavailable: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
}
async function launchChrome(profile) {
  const executable =
    process.env.CHROME_BIN ??
    [
      "C:/Program Files/Google/Chrome/Application/chrome.exe",
      "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
    ].find(existsSync);
  if (!executable)
    fail("Chrome or Edge executable was not found. Set CHROME_BIN.");
  const child = spawn(
    executable,
    [
      "--headless=new",
      "--disable-gpu",
      "--no-first-run",
      "--no-default-browser-check",
      "--remote-debugging-port=0",
      `--user-data-dir=${profile}`,
      "about:blank",
    ],
    { stdio: "ignore", windowsHide: true },
  );
  const activePort = join(profile, "DevToolsActivePort");
  await waitForFile(activePort, "Chrome DevTools endpoint");
  const [port] = (await readFile(activePort, "utf8")).trim().split(/\r?\n/);
  return { child, debuggerBase: `http://127.0.0.1:${port}` };
}
async function waitForFile(path, description) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (existsSync(path)) return;
    await wait(100);
  }
  fail(`Timed out waiting for ${description}.`);
}
async function closeChrome(instance) {
  if (!instance?.child || instance.child.exitCode !== null) return;
  instance.child.kill();
  await Promise.race([
    new Promise((resolvePromise) =>
      instance.child.once("exit", resolvePromise),
    ),
    wait(5_000),
  ]);
}
async function removeOwnedChromeProfile(profile) {
  const root =
    `${chromeProfileRoot}${process.platform === "win32" ? "\\" : "/"}`.toLowerCase();
  if (!profile.toLowerCase().startsWith(root))
    fail("Refusing to remove a Chrome profile outside the E2E root.");
  let cleanupError = null;
  for (let attempt = 0; attempt < 10; attempt += 1) {
    try {
      rmSync(profile, { recursive: true, force: true });
      if (!existsSync(profile)) return;
      cleanupError = new Error(
        "Owned Chrome profile remained after recursive removal.",
      );
    } catch (error) {
      cleanupError = error;
    }
    const retryable =
      cleanupError?.code === "EPERM" ||
      cleanupError?.code === "EBUSY" ||
      cleanupError?.code === "ENOTEMPTY";
    if (!retryable || attempt === 9) throw cleanupError;
    await wait(100 * (attempt + 1));
  }
}
async function connectCdp(debuggerBase) {
  let targets = await (await fetch(`${debuggerBase}/json/list`)).json();
  let target = targets.find((item) => item.type === "page");
  if (!target) {
    target = await (
      await fetch(`${debuggerBase}/json/new?about:blank`, { method: "PUT" })
    ).json();
    targets = [target];
  }
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolvePromise, reject) => {
    socket.addEventListener("open", resolvePromise, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });
  let sequence = 0;
  const pending = new Map();
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(String(event.data));
    if (!message.id) return;
    const request = pending.get(message.id);
    if (!request) return;
    pending.delete(message.id);
    if (message.error)
      request.reject(
        new Error(`CDP ${message.error.code}: ${message.error.message}`),
      );
    else request.resolve(message.result ?? {});
  });
  return {
    send(method, params = {}) {
      const id = ++sequence;
      socket.send(JSON.stringify({ id, method, params }));
      return new Promise((resolvePromise, reject) =>
        pending.set(id, { resolve: resolvePromise, reject }),
      );
    },
    close: async () => socket.close(),
  };
}
