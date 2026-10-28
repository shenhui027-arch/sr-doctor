// Real-Profile walkthrough (v0.6-04): imports official 3.5.14 multi-BE and
// Profile-state fixtures through the offline UI of an already running local
// API, captures 1440x1024 light screenshots of Doctor / Recommendations /
// Compare, and asserts what an on-site engineer needs to read the cause of a
// slow query: the "why" measurement of each finding, the plan node it points
// at, no internal identifiers in visible text, and actionable fail-closed copy.
// v0.6-07 adds the P2 checks: one count form in evidence rows, readable node
// inspector source lines and limits, one plan-node notation, findings grouped
// by node, and readable Compare metric names with before/after measurements.
//
// Usage: node tests/e2e/real-profile-walkthrough.mjs --base-url http://127.0.0.1:18151
//          --out <dir> [--no-assert]
// Offline only; it never touches a StarRocks connection.
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const baseUrl = option("--base-url") ?? "http://127.0.0.1:18151";
const outDir = resolve(option("--out") ?? join(tmpdir(), "real-profile-walkthrough"));
const assertMode = !process.argv.includes("--no-assert");
const repo = resolve(import.meta.dirname, "../..");
const corpus = join(repo, "fixtures/runtime/3.5.14/oracle-corpus-multi-be");
const states = join(repo, "fixtures/runtime/3.5.14/profile-states");

const SAMPLES = [
  { id: "c15", file: join(corpus, "c15_skewed_shuffle_join.profile.txt"), why: "SR3514.OPERATOR_TIME_SKEW" },
  { id: "c16", file: join(corpus, "c16_join_fanout.profile.txt"), why: "SR3514.JOIN_PROBE_FANOUT", inspectNode: true, group: { node: "HASH_JOIN #3", count: 3 } },
  { id: "c17", file: join(corpus, "c17_large_broadcast.profile.txt"), why: "SR3514.BROADCAST_EXCHANGE_BYTES", inspectNode: true },
  { id: "c18", file: join(corpus, "c18_high_cardinality_agg.profile.txt"), why: "SR3514.STREAMING_PREAGG_INEFFECTIVE" },
  { id: "c20", file: join(corpus, "c20_window_sort.profile.txt"), why: "SR3514.NODE_TIME_SHARE" },
  { id: "c21", file: join(corpus, "c21_mbe_broadcast_default_passthrough.profile.txt"), why: "SR3514.BROADCAST_EXCHANGE_BYTES", inspectNode: true },
  { id: "c22", file: join(corpus, "c22_mbe_tablet_skew_scan.profile.txt"), why: "SR3514.OPERATOR_TIME_SKEW" },
  { id: "s01", file: join(states, "s01_level2_shuffle_join.profile.txt"), failClosed: "level_2" },
  { id: "s02", file: join(states, "s02_level2_multi_instance_agg.profile.txt"), failClosed: "level_2" },
  { id: "s03", file: join(states, "s03_running_snapshot.profile.txt"), failClosed: "running" },
  { id: "s04", file: join(states, "s04_running_query_final.profile.txt") },
];

// Visible text must not leak backend codes: snake_case reason ids, rule ids
// with versions, or operator-group / metric references.
const INTERNAL_ID = /\b[a-z]+(?:_[a-z0-9]+){2,}\b|SR3514\.[A-Z_]+@|operator-group:|metric:operator|rule-trace:|finding:SR/;
// StarRocks session variables are what the user types, not internal codes.
const SESSION_VARIABLES = /\b(?:pipeline_profile_level|broadcast_row_limit|streaming_preaggregation_mode)\b/g;

const receipt = { base_url: baseUrl, started_at: new Date().toISOString(), samples: [], compare: null, failures: [], screenshots: [] };
mkdirSync(outDir, { recursive: true });

const profileDir = await mkdtemp(join(tmpdir(), "rpw-chrome-"));
const chrome = await launchChrome(profileDir);
let cdp;
try {
  cdp = await connectCdp(chrome.debuggerBase);
  await cdp.send("Page.enable");
  await cdp.send("Runtime.enable");
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: 1440, height: 1024, deviceScaleFactor: 1, mobile: false });
  await cdp.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "light" }] });

  for (const sample of SAMPLES) receipt.samples.push(await walkSample(sample));
  receipt.compare = await walkCompare(SAMPLES.find((s) => s.id === "c17"), SAMPLES.find((s) => s.id === "c21"));
} finally {
  await cdp?.close();
  chrome.child.kill();
  await wait(500);
  await rm(profileDir, { recursive: true, force: true }).catch(() => {});
}

receipt.finished_at = new Date().toISOString();
receipt.status = receipt.failures.length ? "FAIL" : "PASS";
writeFileSync(join(outDir, "walkthrough-receipt.json"), JSON.stringify(receipt, null, 2));
console.log(`REAL_PROFILE_WALKTHROUGH_STATUS=${receipt.status} screenshots=${receipt.screenshots.length} failures=${receipt.failures.length}`);
for (const failure of receipt.failures) console.log(`  FAIL ${failure}`);
process.exit(receipt.failures.length ? 1 : 0);

async function walkSample(sample) {
  const record = { id: sample.id, fixture_sha256: sha256(readFileSync(sample.file)), views: {} };
  await openFresh();
  await importProfile(sample.file);
  await waitFor(`!!document.querySelector(".doctor-layout")`, `${sample.id} Doctor`);
  await wait(1500); // graph layout worker
  record.views.doctor = await doctorProjection();
  await shot(`${sample.id}-doctor`);

  if (sample.why) {
    const selected = await evaluate(`(() => {
      const button = [...document.querySelectorAll(".finding-item")].find((item) => item.getAttribute("aria-label") === ${JSON.stringify(`Finding ${sample.why}`)});
      if (!button) return false; button.click(); return true; })()`);
    expect(selected, `${sample.id}: finding ${sample.why} listed`);
    await wait(400);
    record.views.why = await doctorProjection();
    await shot(`${sample.id}-why-finding`);
    if (assertMode) assertWhy(sample, record.views.why);
    // Finding -> node: "Show in execution graph" opens the node and keeps this finding.
    await evaluate(`[...document.querySelectorAll(".finding-inspector button")].find((b) => /Show in execution graph|在图中定位/.test(b.innerText))?.click()`);
    await wait(500);
    record.views.located = await doctorProjection();
    await shot(`${sample.id}-why-located`);
    if (assertMode) assertLocated(sample, record.views.why, record.views.located);
    if (sample.inspectNode) {
      // Node inspector disclosures: the original Profile lines and summary limits.
      await evaluate(`document.querySelectorAll(".node-inspector details").forEach((item) => { item.open = true; })`);
      await wait(300);
      record.views.node_inspector = await evaluate(`(() => ({
        text: document.querySelector(".node-inspector")?.innerText ?? "",
        source_buttons: [...document.querySelectorAll(".node-inspector .inspector-source button")].map((button) => button.innerText),
      }))()`);
      await evaluate(`document.querySelector(".node-inspector .inspector-source")?.scrollIntoView({ block: "center" })`);
      await wait(300);
      await shot(`${sample.id}-node-inspector`);
      if (assertMode) assertNodeInspector(sample, record.views.node_inspector);
    }
    if (sample.group && assertMode) assertGroup(sample, record.views.why);
  }
  if (sample.failClosed && assertMode) assertFailClosed(sample, record.views.doctor);
  if (assertMode) assertNoInternalIds(sample.id, "doctor", record.views.doctor.visible_text);

  await clickNav("Recommendations");
  await wait(500);
  record.views.recommendations = { visible_text: await evaluate(`document.querySelector("main")?.innerText ?? ""`) };
  await shot(`${sample.id}-recommendations`);
  if (assertMode) {
    assertNoInternalIds(sample.id, "recommendations", record.views.recommendations.visible_text);
    // One plan node notation across Doctor and Recommendations (v0.7 boards): "Plan N".
    const hash = /(?:^|\s)#\d+\b/m.exec(record.views.recommendations.visible_text);
    expect(!hash, `${sample.id}/recommendations: plan nodes are written "Plan N"${hash ? ` (got "${hash[0].trim()}")` : ""}`);
    if (sample.why) expect(/\bPlan \d+\b/.test(record.views.recommendations.visible_text), `${sample.id}/recommendations: names the plan node as "Plan N"`);
  }
  return record;
}

async function walkCompare(baseline, candidate) {
  await openFresh();
  await clickNav("Compare");
  await waitFor(`!!document.querySelector(".offline-compare-form")`, "Compare form");
  await setFile(0, baseline.file, ".offline-compare-form");
  await setFile(1, candidate.file, ".offline-compare-form");
  await evaluate(`[...document.querySelectorAll(".offline-compare-form button[type=submit]")][0].click()`);
  try {
    await waitFor(`!!document.querySelector(".compare-workspace .compare-main")`, "Compare result", 60_000);
  } catch (error) {
    await shot("compare-timeout");
    throw error;
  }
  await wait(800);
  const visible = await evaluate(`document.querySelector("main")?.innerText ?? ""`);
  await shot("c17-vs-c21-compare");
  const metricNames = await evaluate(`[...document.querySelectorAll(".compare-metric-name strong")].map((item) => item.innerText)`);
  const findingChanges = await evaluate(`[...document.querySelectorAll(".compare-finding-list li")].map((item) => item.innerText.replace(/\\s+/g, " "))`);
  await evaluate(`document.querySelector(".compare-findings")?.scrollIntoView({ block: "start" })`);
  await wait(300);
  await shot("c17-vs-c21-compare-findings");
  // Expand the first operator group so its counter names are on screen.
  await evaluate(`(() => { const group = document.querySelector(".compare-operator-group"); if (group) { group.open = true; group.scrollIntoView({ block: "start" }); } })()`);
  await wait(300);
  await shot("c17-vs-c21-compare-operators");
  const operatorNames = await evaluate(`[...document.querySelectorAll(".compare-operators .compare-metric-name strong")].map((item) => item.innerText)`);
  if (assertMode) {
    assertNoInternalIds("compare", "compare", visible);
    // A metric id is one CamelCase token; a readable name has a space or CJK text.
    const raw = [...metricNames, ...operatorNames].filter((name) => /^[A-Z][A-Za-z0-9]+$/.test(name) && /[a-z][A-Z]/.test(name));
    expect(raw.length === 0, `compare: metric names are readable${raw.length ? ` (raw: ${[...new Set(raw)].slice(0, 6).join(", ")})` : ""}`);
    const broadcast = findingChanges.find((line) => /Broadcast volume/.test(line));
    expect(!!broadcast && /576\.647 MB\s*→\s*384\.519 MB/.test(broadcast), `compare: broadcast change shows 576.647 → 384.519 MB (got "${broadcast ?? "none"}")`);
  }
  return { baseline: baseline.id, candidate: candidate.id, visible_text: visible, metric_names: metricNames, operator_metric_names: operatorNames, finding_changes: findingChanges };
}

async function doctorProjection() {
  return evaluate(`(() => {
    const text = (selector) => document.querySelector(selector)?.innerText ?? "";
    // A grouped card is named by its group heading.
    const cards = [...document.querySelectorAll(".finding-item")].map((item) => ({
      rule: (item.getAttribute("aria-label") ?? "").replace(/^Finding /, ""),
      group: item.closest(".finding-group")?.querySelector(".finding-group-head")?.innerText ?? null,
      text: [item.closest(".finding-group")?.querySelector(".finding-group-head")?.innerText, item.innerText].filter(Boolean).join("\\n"),
    }));
    const selectedNode = document.querySelector(".react-flow__node.selected")?.innerText ?? null;
    const planId = (element) => element?.querySelector("header span:last-child")?.innerText ?? null;
    return {
      visible_text: [".doctor-summary", ".profile-state-notice", ".findings-panel", ".evidence-gaps", ".finding-inspector", ".doctor-graph .graph-state, .doctor-graph .execution-graph-empty"].map(text).join("\\n"),
      cards,
      inspector: text(".finding-inspector"),
      evidence_gaps: text(".evidence-gaps"),
      graph: text(".doctor-graph"),
      selected_node: selectedNode,
      focus_node_id: planId(document.querySelector(".plan-card.finding-focus")),
      inspector_title: text(".finding-inspector h2"),
      node_inspector: text(".node-inspector"),
      // Visually clipped facts keep their full innerText; compare box widths instead.
      facts_clipped: [...document.querySelectorAll(".finding-inspector .inspector-facts dd, .finding-inspector .inspector-facts dt")].some((cell) => cell.scrollWidth > cell.clientWidth + 1),
    };
  })()`);
}

function assertWhy(sample, view) {
  const card = view.cards.find((item) => item.rule === sample.why);
  expect(card, `${sample.id}: why finding card present`);
  if (!card) return;
  // The card names its plan node by the same "#id" the graph shows.
  expect(/#\d+/.test(card.text), `${sample.id}: ${sample.why} card names its plan node id`);
  // A "why" finding leads with its own measurement, not the node's time share.
  if (sample.why !== "SR3514.NODE_TIME_SHARE") {
    expect(!card.text.includes("Share of operator time"), `${sample.id}: ${sample.why} card does not lead with time share`);
    expect(/rule: [>≥]/.test(card.text) && /[×%]|\d (KB|MB|GB|TB)\b/.test(card.text), `${sample.id}: ${sample.why} card shows its rule measurement and condition`);
  }
  expect(!view.facts_clipped, `${sample.id}: inspector facts not clipped`);
  // Evidence rows use one count form; the FE dual form "1.000M (1000000)" is gone.
  const dual = /\d\.\d{3}[KMB] \(\d+\)/.exec(view.inspector);
  expect(!dual, `${sample.id}: evidence counts use one form${dual ? ` (got "${dual[0]}")` : ""}`);
  // Finding -> graph: the finding's node is highlighted under the same "#id".
  const cardId = /#\d+/.exec(card.text)?.[0] ?? null;
  expect(cardId !== null && view.focus_node_id === cardId, `${sample.id}: graph highlights the finding's node ${cardId} (got ${view.focus_node_id})`);
}

function assertLocated(sample, before, after) {
  expect(after.inspector_title === before.inspector_title, `${sample.id}: Show in execution graph keeps the selected finding`);
  const cardId = /#\d+/.exec(before.cards.find((item) => item.rule === sample.why)?.text ?? "")?.[0];
  expect(!!cardId && after.node_inspector.includes(cardId), `${sample.id}: node inspector opens ${cardId}`);
  // Node -> findings: the node inspector lists the finding back.
  const category = before.inspector_title.split(" · ")[0];
  expect(after.node_inspector.includes(category), `${sample.id}: node inspector lists "${category}"`);
}

function assertNodeInspector(sample, view) {
  // A text Profile names every reference by its line; never "text:885" or a bare ordinal.
  const raw = view.source_buttons.filter((label) => !/^(?:Line \d+|第 \d+ 行)/.test(label));
  expect(view.source_buttons.length > 0 && raw.length === 0, `${sample.id}: node inspector source lines are readable (${view.source_buttons.length} lines, raw: ${raw.slice(0, 3).join(", ")})`);
  assertNoInternalIds(sample.id, "node-inspector", view.text);
}

function assertGroup(sample, view) {
  const grouped = view.cards.filter((card) => card.group?.includes(sample.group.node));
  expect(grouped.length === sample.group.count, `${sample.id}: ${sample.group.count} findings grouped under ${sample.group.node} (got ${grouped.length})`);
}

function assertFailClosed(sample, view) {
  const all = `${view.visible_text}\n${view.graph}`;
  const next = sample.failClosed === "level_2"
    ? /pipeline_profile_level\s*=\s*1|Level 1/i
    : /after the query (finishes|has finished)|finished query|final Profile/i;
  expect(next.test(all), `${sample.id}: fail-closed copy says what to do next`);
}

function assertNoInternalIds(id, view, visible) {
  // Rule details and the original Profile are deliberate technical disclosures.
  const leak = visible.split("\n").find((line) => INTERNAL_ID.test(line.replace(SESSION_VARIABLES, "")));
  expect(!leak, `${id}/${view}: no internal identifier in visible text${leak ? ` (${leak.slice(0, 120)})` : ""}`);
}

function expect(condition, message) {
  if (!condition) receipt.failures.push(message);
}

async function openFresh() {
  await cdp.send("Page.navigate", { url: baseUrl });
  await waitFor(`document.readyState === "complete" && !!document.querySelector("header, .app-shell, main")`, "app shell");
  await wait(300);
}

async function importProfile(file) {
  await evaluate(`(() => { const button = [...document.querySelectorAll("button")].find((b) => /^(Import profile|导入 Profile)$/.test(b.innerText.trim())); button?.click(); return !!button; })()`);
  await waitFor(`!!document.querySelector(".import-panel input[type=file]")`, "import panel");
  await setFile(0, file, ".import-panel");
  await evaluate(`document.querySelector(".import-panel button[type=submit]").click()`);
}

async function setFile(index, file, scope = "") {
  const content = await readFile(file, "utf8");
  const name = file.split(/[\\/]/).pop();
  const ok = await evaluate(`(() => {
    const input = document.querySelectorAll(${JSON.stringify(`${scope} input[type=file]`.trim())})[${index}];
    if (!input) return false;
    const transfer = new DataTransfer();
    transfer.items.add(new File([${JSON.stringify(content)}], ${JSON.stringify(name)}, { type: "text/plain" }));
    input.files = transfer.files;
    input.dispatchEvent(new Event("change", { bubbles: true }));
    return true; })()`);
  if (!ok) throw new Error(`file input ${index} missing for ${name}`);
  await wait(200);
}

async function clickNav(label) {
  const ok = await evaluate(`(() => { const tab = [...document.querySelectorAll("nav button, nav a, header button")].find((b) => b.innerText.replace(/\\s+/g, " ").includes(${JSON.stringify(label)})); tab?.click(); return !!tab; })()`);
  if (!ok) throw new Error(`nav ${label} missing`);
}

async function shot(name) {
  const image = await cdp.send("Page.captureScreenshot", { format: "png" });
  const path = join(outDir, `${name}.png`);
  writeFileSync(path, Buffer.from(image.data, "base64"));
  receipt.screenshots.push({ name, path, sha256: sha256(readFileSync(path)) });
}

async function evaluate(expression) {
  const result = await cdp.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (result.exceptionDetails) throw new Error(`evaluate failed: ${result.exceptionDetails.exception?.description ?? result.exceptionDetails.text}`);
  return result.result.value;
}

async function waitFor(expression, description, timeout = 30_000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await evaluate(expression)) return;
    await wait(150);
  }
  throw new Error(`Timed out waiting for ${description}`);
}

function sha256(buffer) {
  return createHash("sha256").update(buffer).digest("hex");
}

function wait(ms) {
  return new Promise((done) => setTimeout(done, ms));
}

function option(name) {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : undefined;
}

async function launchChrome(profile) {
  const executable = process.env.CHROME_BIN ?? [
    "C:/Program Files/Google/Chrome/Application/chrome.exe",
    "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  ].find(existsSync);
  if (!executable) throw new Error("Chrome or Edge executable was not found. Set CHROME_BIN.");
  const child = spawn(executable, ["--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "about:blank"], { stdio: "ignore", windowsHide: true });
  const activePort = join(profile, "DevToolsActivePort");
  const deadline = Date.now() + 30_000;
  while (!existsSync(activePort)) {
    if (Date.now() > deadline) throw new Error("Chrome DevTools endpoint did not start");
    await wait(100);
  }
  await wait(100);
  const [port] = (await readFile(activePort, "utf8")).trim().split(/\r?\n/);
  return { child, debuggerBase: `http://127.0.0.1:${port}` };
}

async function connectCdp(debuggerBase) {
  const targets = await (await fetch(`${debuggerBase}/json/list`)).json();
  const target = targets.find((item) => item.type === "page") ?? await (await fetch(`${debuggerBase}/json/new?about:blank`, { method: "PUT" })).json();
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((done, reject) => {
    socket.addEventListener("open", done, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });
  let sequence = 0;
  const pending = new Map();
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(String(event.data));
    const request = message.id ? pending.get(message.id) : null;
    if (!request) return;
    pending.delete(message.id);
    if (message.error) request.reject(new Error(`CDP ${message.error.code}: ${message.error.message}`));
    else request.resolve(message.result ?? {});
  });
  return {
    send(method, params = {}) {
      const id = ++sequence;
      socket.send(JSON.stringify({ id, method, params }));
      return new Promise((done, reject) => pending.set(id, { resolve: done, reject }));
    },
    close: async () => socket.close(),
  };
}
