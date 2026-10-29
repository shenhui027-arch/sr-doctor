import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";
import { Script } from "node:vm";
import {
  officialAnalysisConnectionId,
  officialProfileIdentityEchoes,
  sameOfficialProfileIdentitySummary,
  summarizeOfficialProfileIdentity,
  summarizeOfficialProfileRequest,
  summarizeOfficialProfileResponse,
  summarizeProfileFetchOfficialIdentity,
} from "./official-path.mjs";

// This file is intentionally a source-preparation check. It does not import
// browser-e2e.mjs, start HTTP/Chrome, or count as a Browser E2E PASS.
const runnerPath = resolve("tests/e2e/browser-e2e.mjs");
const runnerSource = readFileSync(runnerPath, "utf8");
const developmentWrapperPath = resolve("scripts/e2e/task31-development-browser.ps1");
const developmentWrapperSource = readFileSync(developmentWrapperPath, "utf8");

function assertLayoutContains(source, fragment, message) {
  // These source-preparation checks deliberately ignore formatting only. A
  // trailing comma before a closing delimiter is equally non-semantic.
  const removeFormatting = (value) => value.replace(/\s+/g, "").replace(/,([)\]}])/g, "$1");
  assert.ok(
    removeFormatting(source).includes(removeFormatting(fragment)),
    message ?? `Expected source fragment was not present: ${fragment}`,
  );
}

function emittedEvaluateTemplate(functionName, followingFunctionName, bindings = {}) {
  const start = runnerSource.indexOf(`async function ${functionName}`);
  const end = runnerSource.indexOf(`async function ${followingFunctionName}`, start);
  assert.ok(start >= 0 && end > start, `could not isolate ${functionName}`);
  const source = runnerSource.slice(start, end);
  const template = source.trimEnd().match(/await evaluate\(\s*`([\s\S]*?)`\s*,?\s*\);\s*}\s*$/)?.[1];
  assert.ok(template, `could not extract ${functionName} CDP expression`);
  // Compile exactly the string that the outer template literal gives CDP.
  // Parsing browser-e2e.mjs itself does not catch a single escaped slash being
  // consumed before the browser sees a RegExp literal.
  const names = Object.keys(bindings);
  return Function(...names, `return \`${template}\`;`)(...names.map((name) => bindings[name]));
}

function emittedEvaluateTemplateAfter(anchor, bindings) {
  const start = runnerSource.indexOf(anchor);
  assert.ok(start >= 0, `could not find ${anchor}`);
  const template = runnerSource.slice(start).match(/evaluate\(\s*`([\s\S]*?)`\s*,?\s*\)/)?.[1];
  assert.ok(template, `could not extract CDP expression after ${anchor}`);
  const names = Object.keys(bindings);
  return Function(...names, `return \`${template}\`;`)(...names.map((name) => bindings[name]));
}

test("family detail telemetry records shape and roles without raw identities or evidence", () => {
  assertLayoutContains(runnerSource, 'if (project === "connected-audit-docker") await auditFamilyDoctorCoverage();');
  const projection = runnerSource.slice(runnerSource.indexOf("family_detail: pathname"), runnerSource.indexOf("runtime: report ?"));
  assert.match(projection, /representative_states:/);
  assert.match(projection, /bucket_count:/);
  assert.doesNotMatch(projection, /query_id:|family_key:|native_digest:|statement:/);
  assertLayoutContains(runnerSource, "pathname.endsWith('/audit/families') || pathname.endsWith('/audit/family-detail')");
});

test("browser runner declares the current six audit windows and bounded reset", () => {
  assertLayoutContains(runnerSource, 'const expectedAuditWindowPresets = Object.freeze(["1h", "12h", "1d", "3d", "7d", "30d"]);');
  assertLayoutContains(runnerSource, 'setInputValue(\'select[aria-label="Audit window"]\', "30d")');
  assertLayoutContains(runnerSource, 'setInputValue(\'select[aria-label="Audit window"]\', originalWindowPreset)');
  assert.match(runnerSource, /auditWindowSecondsByPreset/);
  assert.match(runnerSource, /window_seconds === 2592000/);
  assert.doesNotMatch(runnerSource, /"15m"|"24h"/);
});

test("browser runner verifies Overview through its current endpoint, not the old read chain", () => {
  const overviewCoverage = runnerSource.match(/async function auditWorkloadCoverage\(\) \{([\s\S]*?)\nasync function auditFilterCoverage\(\)/)?.[1];
  assert.ok(overviewCoverage, "could not locate the bounded Overview coverage block");
  const overviewOnly = overviewCoverage.split("  const queryRequestsBefore")[0];
  assert.match(runnerSource, /const auditOverviewRoute = "\/audit\/overview";/);
  assert.match(runnerSource, /routeTelemetry\(auditOverviewRoute\)/);
  assert.match(runnerSource, /pathname\.endsWith\('\/audit\/overview'\)/);
  assert.match(runnerSource, /audit_overview_http200_full_window_cards_coverage/);
  // The Overview part ends where it records its cards; the family drill that follows owns the member read.
  const overviewPart = overviewOnly.split("audit_overview_http200_full_window_cards_coverage")[0];
  assert.ok(overviewPart.includes("root?.querySelectorAll('.audit-kpi-grid .audit-kpi').length"));
  assert.ok(overviewPart.includes('overviewTop.full_window_count === overviewCoverage.retained_family_count'));
  assert.ok(overviewPart.includes('receipt.audit_overview.query_count === overviewCoverage.retained_event_count'));
  assert.doesNotMatch(overviewPart, /routeTelemetry\("\/audit\/occurrences"\)/, "Overview coverage used the member-ranking endpoint");
});

test("static preparation keeps fetch telemetry separate from interception or mock injection", () => {
  for (const forbiddenToken of ["page.route(", "route.fulfill(", "route.continue(", "page.on('request", "page.on(\"request"]) {
    assert.equal(runnerSource.includes(forbiddenToken), false, `runner mixed interception layer: ${forbiddenToken}`);
  }
  assert.match(runnerSource, /window\.fetch = async \(input, init\)/);
});

test("official controlled transport is endpoint-scoped, receipt-redacted, and restored", () => {
  assert.match(runnerSource, /officialProfileTransport/);
  assert.match(runnerSource, /controlled_mock_over_real_session/);
  assert.match(runnerSource, /if \(!isOfficial\(input\)\) return previous\(input, init\)/);
  assert.match(runnerSource, /__doctorE2eOfficialMockBypass/);
  assert.match(runnerSource, /finally \{[\s\S]*?if \(mockInstalled\)[\s\S]*?restoreOfficialMock\(\)/);
  assert.match(runnerSource, /officialRequestLog\(\)/);
  assert.match(runnerSource, /officialResponseLog\(\)/);
  assert.match(runnerSource, /__doctorE2eOfficialConnectionId/);
  assert.match(runnerSource, /endpoint: 'official_analysis'/);
  assert.match(runnerSource, /display_hash: response\.display_hash/);
  assert.match(runnerSource, /source_hash: response\.source_hash/);
  assert.match(runnerSource, /full_connection_id_visible/);
  const receiptProjection = runnerSource.slice(runnerSource.indexOf("result.responses.push({"), runnerSource.indexOf("\nasync function installOfficialMock"));
  assert.doesNotMatch(receiptProjection, /display_text/);
  assert.match(runnerSource, /__doctorE2eOfficialNodeCandidates/);
  assert.match(runnerSource, /node_ids\) === JSON\.stringify\(expectedNodeIds\)/);
  assert.match(runnerSource, /message_id: "request_failed"[\s\S]*?recoverability: "retry_same"/);
  assert.match(runnerSource, /message_id: "remote_timeout"[\s\S]*?recoverability: "restart"/);
  assert.match(runnerSource, /identitySwitchLateReplyCoverage/);
  assert.match(runnerSource, /=== 'Diagnose'/);
  assert.match(runnerSource, /query_identity_switch_aborts_and_drops_late_official_reply/);
  assert.match(runnerSource, /identity_switch_profile_fetch_attempts/);
  assert.match(runnerSource, /profileFetchErrors/);
  assert.match(runnerSource, /query_id_sha256/);
  assert.match(runnerSource, /execution_attempt_id_sha256/);
  assert.match(runnerSource, /official_profile_eligibility/);
  const officialLedger = runnerSource.slice(runnerSource.indexOf("async function installRouteTelemetry"), runnerSource.indexOf("async function routeTelemetry"));
  assert.doesNotMatch(officialLedger, /raw_path:/);
  assert.match(officialLedger, /path_connection_id_sha256/);
  assert.match(officialLedger, /connection_id_sha256/);
  assert.match(runnerSource, /identitySwitchLateReplyCoverage[\s\S]*?clickButton\("Official reading"\)/);
  const recoveryBlock = runnerSource.slice(runnerSource.indexOf("const recoveryBefore"), runnerSource.indexOf("const recoveredRequest"));
  assert.ok(recoveryBlock.indexOf("const recoveryResponseBefore") < recoveryBlock.indexOf('clickButton("Read official analysis")'), "recovery response baseline must be captured before the request click");
 });

test("identity switching expands only the real bounded Audit window and uses an optional opaque retained hash allowlist", () => {
  assert.match(runnerSource, /E2E_RETAINED_PROFILE_HASHES/);
  assert.match(runnerSource, /parseSha256Allowlist/);
  assert.match(runnerSource, /E2E_AUDIT_FIXTURE_WINDOW/);
  assert.match(runnerSource, /resolveAuditFixtureWindow/);
  assert.match(runnerSource, /applyAuditFixtureWindowAfterOverview/);
  assert.match(runnerSource, /const identityAuditWindow = auditFixtureWindow \?\? "1h"/);
  assert.match(runnerSource, /const originalWindowPreset = await evaluate/);
  assertLayoutContains(runnerSource, 'setInputValue(\'select[aria-label="Audit window"]\', originalWindowPreset)');
  assertLayoutContains(runnerSource, "const identityAuditWindowSeconds = auditWindowSecondsByPreset[identityAuditWindow]");
  assertLayoutContains(runnerSource, "window_seconds === identityAuditWindowSeconds");
  assert.match(runnerSource, /select\[aria-label="Top limit"\].*500/s);
  assert.match(runnerSource, /audit_top: auditTop/);
  assert.match(runnerSource, /returned_count/);
  assert.match(runnerSource, /truncated/);
  assert.match(runnerSource, /identity_switch_audit_window/);
  assert.match(runnerSource, /allowlistedCandidates/);
  assert.match(runnerSource, /INCONCLUSIVE: E2E_RETAINED_PROFILE_HASHES/);
  assert.match(runnerSource, /no Profile fetch attempted/);
  assert.doesNotMatch(runnerSource, /E2E_RETAINED_PROFILE_HASHES\s*=\s*[\"'][^\"']+[\"']/);
 });

test("browser evaluation failures retain CDP exception detail for controlled-B diagnosis", () => {
  const evaluateBlock = runnerSource.slice(runnerSource.indexOf("async function evaluate(expression)"), runnerSource.indexOf("async function waitFor(check"));
  assertLayoutContains(evaluateBlock, "const exception = detail.exception?.description ?? detail.exception?.value ?? detail.text");
  assert.match(evaluateBlock, /Browser evaluation failed: \$\{exception\}\$\{location\}/);
});

test("browser runner waits for a transient owned Chrome profile lock instead of losing a passing receipt", () => {
  assert.match(runnerSource, /if \(chromeProfile\) await removeOwnedChromeProfile\(chromeProfile\);/);
  const cleanupBlock = runnerSource.slice(runnerSource.indexOf("async function removeOwnedChromeProfile"), runnerSource.indexOf("async function connectCdp"));
  assert.match(cleanupBlock, /for \(let attempt = 0; attempt < 10; attempt \+= 1\)/);
  assertLayoutContains(cleanupBlock, 'cleanupError?.code === "EPERM" || cleanupError?.code === "EBUSY" || cleanupError?.code === "ENOTEMPTY"');
  assert.match(cleanupBlock, /await wait\(100 \* \(attempt \+ 1\)\)/);
  assert.match(cleanupBlock, /if \(!existsSync\(profile\)\) return;/);
});

test("fixed controlled B compiles each newly emitted CDP expression before browser execution", () => {
  const drawerBlock = runnerSource.slice(runnerSource.indexOf("async function auditFilterDrawerState"), runnerSource.indexOf("async function prepareAuditFixtureFilterControls"));
  const drawerExpression = drawerBlock.match(/return evaluate\(\s*`([\s\S]*?)`\s*,?\s*\);/)?.[1];
  const summaryExpression = runnerSource.match(/await clickElement\(\s*'([^']+\.audit-filter-bar button\.audit-more-filters[^']+)'/)?.[1];
  const selectedWindowSource = runnerSource.match(/const selectedWindow = await evaluate\(\s*"([\s\S]*?)"\s*,?\s*\);/)?.[1];
  const applyExpression = runnerSource.match(/await clickElement\(\s*`([^`]+audit-filter-bar button[^`]+)`/)?.[1];
  assert.ok(drawerExpression && summaryExpression && selectedWindowSource && applyExpression, "could not extract the controlled-B CDP expressions");
  const selectedWindowExpression = JSON.parse(`"${selectedWindowSource}"`);
  for (const [name, expression] of [
    ["drawer state", drawerExpression],
    ["More filters", summaryExpression],
    ["selected window", selectedWindowExpression],
    ["Apply filters", applyExpression],
  ]) {
    assert.doesNotThrow(() => new Script(expression), `${name} expression must compile before CDP evaluation`);
  }
});

test("route telemetry compiles the exact template emitted to CDP", () => {
  const expression = emittedEvaluateTemplate("installRouteTelemetry", "routeTelemetry", { isolatedFixtureSql: "fixed isolated SQL" });
  assert.doesNotThrow(() => new Script(expression), "route telemetry CDP expression must compile after template escaping");
  assert.match(expression, /\^\\\/api\\\/v1\\\/connections\\\//, "emitted official-route regexp lost its slash escaping");
});

test("official endpoint parser binds encoded connection IDs only on the exact route", () => {
  assert.equal(
    officialAnalysisConnectionId("/api/v1/connections/connection%2Fone/profiles/official-analysis"),
    "connection/one",
  );
  assert.equal(officialAnalysisConnectionId("/api/v1/connections/connection%2Fone/profiles/fetch"), null);
  assert.equal(officialAnalysisConnectionId("/api/v1/connections/connection%2Fone/profiles/official-analysis/extra"), null);
  assert.equal(officialAnalysisConnectionId("/api/v1/connections/%E0%A4%A/profiles/official-analysis"), null);
});

const opaque = (value) => `sha256:${value}`;

test("official identity helpers accept exactly discovery and verified standard attempt arms", () => {
  const discovery = summarizeOfficialProfileIdentity({ kind: "discovery", discovery_generation: 7 }, opaque);
  const standard = summarizeOfficialProfileIdentity({ kind: "verified_standard_attempt", execution_attempt_id: "attempt-17" }, opaque);
  assert.deepEqual(discovery, { kind: "discovery", discovery_generation: 7, execution_attempt_id_sha256: null });
  assert.deepEqual(standard, { kind: "verified_standard_attempt", discovery_generation: null, execution_attempt_id_sha256: "sha256:attempt-17" });
  assert.equal(sameOfficialProfileIdentitySummary(discovery, standard), false);

  const discoveredFetch = summarizeProfileFetchOfficialIdentity({ query: { connection_id: "connection-a", query_id: "query-a", discovery_generation: 7 } }, opaque);
  const standardFetch = summarizeProfileFetchOfficialIdentity({
    query: { connection_id: "connection-a", query_id: "query-a", discovery_generation: null },
    official_profile_eligibility: { execution_attempt_id: "attempt-17", method: "standard_profile" },
  }, opaque);
  assert.equal(discoveredFetch.identity.kind, "discovery");
  assert.equal(standardFetch.identity.kind, "verified_standard_attempt");
  assert.equal(standardFetch.identity.execution_attempt_id_sha256, "sha256:attempt-17");
  const standardRequest = summarizeOfficialProfileRequest({
    connection_id: "connection-a", query_id: "query-a", identity: { kind: "verified_standard_attempt", execution_attempt_id: "attempt-17" }, node_ids: [],
  }, opaque);
  const standardResponse = summarizeOfficialProfileResponse({
    kind: "analyze_profile_text", query: { connection_id: "connection-a", query_id: "query-a", discovery_generation: null },
    identity: { kind: "verified_standard_attempt", execution_attempt_id: "attempt-17" }, node_ids: [],
  }, opaque);
  assert.ok(standardRequest && standardResponse);
  assert.equal(officialProfileIdentityEchoes(standardRequest, standardResponse), true);
});

test("official identity helpers reject missing, mixed, retired, and mismatched identity contracts", () => {
  assert.equal(summarizeOfficialProfileIdentity(undefined, opaque), null);
  assert.equal(summarizeOfficialProfileIdentity({ kind: "discovery", discovery_generation: 7, execution_attempt_id: "attempt-17" }, opaque), null);
  assert.equal(summarizeOfficialProfileIdentity({ kind: "verified_standard_attempt", execution_attempt_id: "attempt-17", discovery_generation: 7 }, opaque), null);
  assert.equal(summarizeOfficialProfileRequest({ connection_id: "connection-a", query_id: "query-a", discovery_generation: 7, node_ids: [] }, opaque), null);
  assert.equal(summarizeOfficialProfileResponse({ kind: "analyze_profile_text", query: { connection_id: "connection-a", query_id: "query-a", discovery_generation: 7 }, node_ids: [] }, opaque), null);
  assert.equal(summarizeProfileFetchOfficialIdentity({
    query: { connection_id: "connection-a", query_id: "query-a", discovery_generation: 7 },
    official_profile_eligibility: { execution_attempt_id: "attempt-17", method: "standard_profile" },
  }, opaque), null);

  const request = summarizeOfficialProfileRequest({
    connection_id: "connection-a", query_id: "query-a", identity: { kind: "discovery", discovery_generation: 7 }, node_ids: [3],
  }, opaque);
  const response = summarizeOfficialProfileResponse({
    kind: "analyze_profile_text", query: { connection_id: "connection-a", query_id: "query-a", discovery_generation: 7 },
    identity: { kind: "discovery", discovery_generation: 7 }, node_ids: [3],
  }, opaque);
  const wrongEcho = summarizeOfficialProfileResponse({
    kind: "analyze_profile_text", query: { connection_id: "connection-a", query_id: "query-a", discovery_generation: 8 },
    identity: { kind: "discovery", discovery_generation: 8 }, node_ids: [3],
  }, opaque);
  const standardWithGeneration = summarizeOfficialProfileResponse({
    kind: "analyze_profile_text", query: { connection_id: "connection-a", query_id: "query-a", discovery_generation: 7 },
    identity: { kind: "verified_standard_attempt", execution_attempt_id: "attempt-17" }, node_ids: [3],
  }, opaque);
  assert.ok(request && response && wrongEcho);
  assert.equal(officialProfileIdentityEchoes(request, response), true);
  assert.equal(officialProfileIdentityEchoes(request, wrongEcho), false);
  assert.equal(standardWithGeneration, null);
});

test("official whole-query scope clears the Runtime Top selection through a real blank React Flow pane", () => {
  const officialCoverage = runnerSource.slice(
    runnerSource.indexOf("async function officialProfileCoverage"),
    runnerSource.indexOf("async function identitySwitchLateReplyCoverage"),
  );
  const paneClear = runnerSource.slice(
    runnerSource.indexOf("async function clickReactFlowPaneBackground"),
    runnerSource.indexOf("async function clickElement"),
  );
  assert.match(officialCoverage, /await clickReactFlowPaneBackground\(flowPaneSelector\)/);
  assert.match(officialCoverage, /selection cleared by actual React Flow pane click/);
  assert.ok(officialCoverage.indexOf("selection cleared by actual React Flow pane click") < officialCoverage.indexOf('clickButton("Official reading")'), "the real pane clear must finish before opening the official dialog");
  assert.doesNotMatch(officialCoverage, /clickSelector\('\.workspace-doctor \.readable-dataflow-panel \.react-flow__pane'\)/);
  assert.match(paneClear, /document\.elementFromPoint\(x, y\)/);
  assert.match(paneClear, /hit === pane/);
  assert.match(paneClear, /Input\.dispatchMouseEvent/);
});

test("runtime duplicate-submit preparation checks UUID presence without persisting its value", () => {
  assert.match(runnerSource, /button\.click\(\); button\.click\(\);/);
  assert.match(runnerSource, /runtime_same_tick_double_submit_one_response_uuid_bound/);
  assert.match(runnerSource, /execution_attempt_id_valid: executionAttemptIdValid/);
  const telemetryExpression = emittedEvaluateTemplate("installRouteTelemetry", "routeTelemetry", { isolatedFixtureSql: "fixed isolated SQL" });
  const controlledRouteProjection = telemetryExpression.slice(
    telemetryExpression.indexOf("const recordControlledRoute"),
    telemetryExpression.indexOf("const hashOpaque"),
  );
  assert.doesNotMatch(controlledRouteProjection, /routes\.push\(\s*\{[\s\S]*?\bexecution_attempt_id\s*:/);
});

test("each screenshot redacts fresh identifiers and narrow Compare explicitly selects Compare", () => {
  assert.match(runnerSource, /async function capture\(name, \{ redactSql = false \} = \{\}\) \{[\s\S]*?await redactVisibleIdentifiersForEvidence\(\{ redactSql \}\);/);
  assert.match(runnerSource, /async function narrowViewportEvidence\(\) \{\s+await clickButton\("Compare"\);/);
});

test("family member navigation requires a fresh bounded discovery, not just a visible summary", () => {
  assert.match(runnerSource, /members\.audit_scope\?\.has_digest_filter/);
  assert.match(runnerSource, /members\.audit_generation > detailProjection\.audit_generation/);
  assert.match(runnerSource, /routes\.length === memberRequestsBefore \+ 1/);
  assert.match(runnerSource, /audit-family-member-rankings-1440x1024/);
});

test("family representative Profile acceptance requires explicit retained fetch without execution", () => {
  assert.match(runnerSource, /Fetch Profile for evidence representative/);
  assert.match(runnerSource, /fetched\.profile_fetch_state === "fetched"/);
  assert.match(runnerSource, /audit_family_evidence_profile_explicit_fetch_no_execution/);
  assert.match(runnerSource, /runtimeBeforeProfile/);
});

test("SQL session summaries participate in screenshot redaction and its visibility check", () => {
  const source = runnerSource.slice(runnerSource.indexOf("async function redactVisibleIdentifiersForEvidence"), runnerSource.indexOf("async function capture("));
  assert.match(source, /sql-analysis-connection code, \.sql-analysis-locked code, \.sql-analysis-confirm-action code, section\[aria-label="Connected session"\] code, section\[aria-label="已连接会话"\] code/);
  assert.ok(source.includes("connectionIdPattern = /^[0-9a-f]{32}(?:[0-9a-f]{16})?$/i"));
  const sessionIdPattern = /^[0-9a-f]{32}(?:[0-9a-f]{16})?$/i;
  assert.equal(sessionIdPattern.test("a".repeat(48)), true);
  assert.equal(sessionIdPattern.test("a".repeat(64)), false);
  assert.match(source, /for \(const valueElement of connectionValueElements\)/);
  assert.match(source, /visibleConnectionIds = \/\\\\b\[0-9a-f\]\{32\}\(\?:\[0-9a-f\]\{16\}\)\?\\\\b\/i\.test\(visibleText\)/);
  assert.match(source, /changed\.push\(\{ node, original, replacement \}\)/);
  assert.match(source, /textarea\[data-sql-analysis-input="sql"\]/);
  assert.match(source, /Native official text redacted for development evidence/);
  assert.match(source, /sql_inputs_redacted/);
  const captureSource = runnerSource.slice(runnerSource.indexOf("async function capture("));
  assert.match(captureSource, /\/\\\\b\[0-9a-f\]\{32\}\(\?:\[0-9a-f\]\{16\}\)\?\\\\b\/i\.test\(document\.body\.innerText\)/);
  const expression = emittedEvaluateTemplateAfter("async function redactVisibleIdentifiersForEvidence", { redactSql: false });
  assert.doesNotThrow(() => new Script(expression));
  const redactionTemplate = source.match(/const redaction = await evaluate\(\s*`([\s\S]*?)`\s*,?\s*\);/)?.[1];
  assert.ok(redactionTemplate, "could not extract screenshot redaction CDP expression");
  const emittedRedaction = Function(`return \`${redactionTemplate}\`;`)();
  assert.doesNotThrow(() => new Script(emittedRedaction));
});

test("SQL input screenshots restore the input viewport after navigation", () => {
  for (const name of ["topology-only-sql-input-1440x1024", "runtime-top-sql-input-1440x1024"]) {
    const capture = runnerSource.indexOf(`await capture("${name}")`);
    assert.ok(capture >= 0, "SQL screenshot must exist.");
    const prefix = runnerSource.slice(0, capture);
    const start = prefix.lastIndexOf("await evaluate(");
    assert.ok(start >= 0, "Screenshot requires viewport preparation.");
    const preparation = prefix.slice(start);
    assert.match(preparation, /data-sql-analysis-input=sql/);
    assert.match(preparation, /scrollIntoView/);
    const template = preparation.match(/await evaluate\(\s*`([\s\S]*?)`\s*,?\s*\)/)?.[1];
    assert.ok(template, "Screenshot viewport preparation must contain a CDP expression.");
    assert.doesNotThrow(() => new Script(template));
  }
});

// Execute only the bounded pure predicate, never the browser entry point.
test("32-hex Audit digest and real session ID are separately redacted without weakening leak guards", () => {
  const source = runnerSource.slice(runnerSource.indexOf("async function redactVisibleIdentifiersForEvidence"), runnerSource.indexOf("async function capture("));
  const template = source.match(/const redaction = await evaluate\(\s*`([\s\S]*?)`\s*,?\s*\);/)[1];
  const expression = Function(`return \`${template}\`;`)();
  const digest = "a".repeat(32), session = "b".repeat(48);
  const makeNode = text => ({ nodeValue: text, isConnected: true, parentElement: { closest: () => null } });
  const digestNode = makeNode(digest), sessionNode = makeNode(session);
  const sessionElement = { get textContent() { return sessionNode.nodeValue; } };
  const body = { get innerText() { return digestNode.nodeValue + " " + sessionNode.nodeValue; } };
  const document = { body,
    createTreeWalker(root) { const nodes = root === body ? [digestNode, sessionNode] : [sessionNode]; let index = -1;
      return { nextNode() { return ++index < nodes.length; }, get currentNode() { return nodes[index]; } }; },
    querySelectorAll(selector) { return selector.includes('.running-profile-connection code') ? [sessionElement] : []; },
  };
  const window = {};
  const result = new Script(expression).runInNewContext({ document, window, NodeFilter: { SHOW_TEXT: 4 } });
  assert.equal(result.full_connection_id_visible, false);
  assert.equal(digestNode.nodeValue, "OPAQUE-001");
  assert.equal(sessionNode.nodeValue, "CID-001");
  assert.equal(window.__doctorE2eScreenshotConnectionAliases.size, 1);
  assert.equal(window.__doctorE2eScreenshotOpaqueAliases.size, 1);
  for (const { node, original, replacement } of [...window.__doctorE2eScreenshotText].reverse())
    if (node.nodeValue === replacement) node.nodeValue = original;
  assert.equal(body.innerText, digest + " " + session);
});

test("coordinator emits success only after teardown and final receipt persistence", () => {
  const source = readFileSync(resolve("scripts/e2e/run-browser-e2e.ps1"), "utf8");
  const outputs = [...source.matchAll(/\$summary \| ConvertTo-Json -Compress/g)];
  assert.equal(outputs.length, 1);
  assert.ok(outputs[0].index > source.indexOf('[IO.File]::WriteAllText($releaseReceiptPath'));
  assert.ok(outputs[0].index > source.indexOf("throw 'Dedicated E2E teardown or postcondition verification failed.'"));
});

test("F05 is readonly opt-in and separate from Top1/navigation coverage", () => {
  assert.match(runnerSource,/project === "connected-readonly-docker" && process\.env\.E2E_MISSING_PROFILE_DIGEST/);
  assert.match(runnerSource,/captureMissingProfileFamily\(\{ evaluate/);
  assert.match(runnerSource,/requiredEnv\("E2E_MISSING_PROFILE_QUERY_ID"\)/);
});


test("owned production runner selects the local API port and restores its environment", () => {
  const source = readFileSync(resolve("scripts/e2e/run-browser-e2e.ps1"), "utf8");
  assert.ok(source.includes('[ValidateRange(1, 65535)][int]$ApiPort = 8000'));
  assert.ok(source.includes('if (-not $DevelopmentRunner) { $env:DOCTOR_PORT = [string]$ApiPort }'));
  assert.ok(source.includes('$env:E2E_BASE_URL = "http://127.0.0.1:$ApiPort"'));
  assert.ok(!source.includes('if ($DevelopmentRunner) { $env:E2E_BASE_URL'));
  assert.ok(source.slice(source.lastIndexOf('finally {')).includes('$env:DOCTOR_PORT = $previousDoctorPort'));
});


test("Overview Top is read from the typed primary arm, not a nonexistent root Top", () => {
  const match = runnerSource.match(/const top = (.*?);      const auditTop = top/);
  assert.ok(match);
  const selectTop = new Function("pathname", "body", `return ${match[1]};`);
  const rootTop = { returned_count: 99 };
  const primaryTop = { returned_count: 3 };
  assert.equal(selectTop("/api/v1/audit/overview", { top: rootTop, primary: { state: "occurrences", top: primaryTop } }), primaryTop);
  assert.equal(selectTop("/api/v1/audit/overview", { top: rootTop }), undefined);
  assert.equal(selectTop("/api/v1/audit/occurrences", { top: rootTop }), rootTop);
  assert.ok(runnerSource.includes('overviewProjection?.audit_primary_state === "families"'));
});


test("every literal browser button label exists in current production UI sources", () => {
  const paths = readdirSync(resolve("frontend/src"), { recursive: true }).filter(path => /\.tsx?$/.test(path) && !path.includes(".test."));
  const ui = paths.map(path => readFileSync(resolve("frontend/src", path), "utf8")).join("\n");
  const labels = new Set([...runnerSource.matchAll(/clickButton\("([^"\n]+)"\)/g)].map(match => match[1]));
  for (const label of labels) assert.ok(ui.includes(JSON.stringify(label)), `Button label absent from current production UI: ${label}`);
  const auditUi = readFileSync(resolve("frontend/src/features/audit/AuditWorkloadWorkspace.tsx"), "utf8");
  assert.ok(auditUi.includes('onClick={() => { setOverviewShown(true); if (mode === "similar") chooseMode("families"); }} type="button">{text("Overview"'));
  for (const [mode, label] of [["occurrences", "Individual executions"], ["families", "Repeated SQL"]]) {
    assert.ok(auditUi.includes(`chooseMode("${mode}"); }} type="button">{text("${label}"`));
    assert.ok(runnerSource.includes(`clickButton("${label}")`));
  }
});


test("Audit diagnosis targets the one-step Diagnose action and its Open diagnosis follow-up, not legacy Profile buttons", () => {
  const ui = readFileSync(resolve("frontend/src/features/audit/AuditWorkloadWorkspace.tsx"), "utf8");
  for (const label of ["Diagnose", "Open diagnosis"]) {
    assert.ok(ui.includes(`text("${label}"`));
    assert.ok(runnerSource.includes(`document.querySelectorAll('.audit-selection-actions button')).find(button => button.textContent.trim() === '${label}' && !button.disabled)`));
  }
  assert.ok(!runnerSource.includes("1. Fetch existing Profile") && !runnerSource.includes("2. Open diagnosis"));
  assert.ok(!runnerSource.includes('clickButton("Query families")'));
  assert.ok(!runnerSource.includes('clickButton("Query rankings")'));
});

test("SQL draft round trip preserves text and plan mode without implicit execution", () => {
  const expression = emittedEvaluateTemplateAfter("const after = await evaluate(", {});
  assert.doesNotThrow(() => new Script(expression));
  assert.match(runnerSource, /after\.sql === before && after\.planMode === planMode/);
  assert.match(runnerSource, /afterCounts\.every\(\(count, index\) => count === counts\[index\]\)/);
  assert.match(runnerSource, /sqlDraftRoundTripCoverage\("standard_profile", "costs"\)/);
  assert.match(expression, /data-plan-mode/);
  assert.doesNotMatch(runnerSource, /data-sql-analysis-method|method-locked/);
  const ui = readFileSync(resolve("frontend/src/features/plan/SqlAnalysisWorkspace.tsx"), "utf8");
  for (const hook of ['data-sql-analysis-input="sql"', 'data-sql-analysis-action="view-plan"', 'data-sql-analysis-action="request-confirmation"', 'data-sql-analysis-action="confirm-execution"', 'data-sql-analysis-plan-result="error"', 'data-sql-analysis-plan-result="success"', "data-sql-analysis-stage={props.stage}", "data-plan-mode={id}"])
    assert.ok(ui.includes(hook), `SQL workspace lost its stable E2E hook ${hook}`);
});

test("retained complex Profile is hash-pinned and verifies the DTO graph, summary and hottest node", () => {
  const coverage = runnerSource.slice(runnerSource.indexOf("async function retainedComplexProfileCoverage"), runnerSource.indexOf("async function auditFilterDrawerState"));
  assert.match(runnerSource, /process\.env\.E2E_COMPLEX_PROFILE_PATH/);
  assert.match(runnerSource, /9538497e592e029e4439f3272a0d05fb81fbe682d488ccd703b3b97d92d1d2ce/);
  assert.match(runnerSource, /live_database_query: false/);
  assertLayoutContains(coverage, 'graph?.node_count === 28 && graph.edge_count === 35 && graph.data_flow_pair_count === 27 && graph.summary_state === "available" && graph.summary_node_count === 28');
  assertLayoutContains(coverage, 'graph.hottest?.node_id === "0" && graph.hottest.basis_points === 1728 && graph.hottest.hotspot === "second_most_consuming"');
  assert.match(coverage, /firstRow\.includes\("17\.28%"\)/);
  assert.match(coverage, /clickButton\("Fit all"\)/);
  assert.ok(coverage.includes("await clickSelector(`${root} .graph-counts`);"));
  assert.match(coverage, /clickReactFlowPaneBackground/);
  assert.match(coverage, /retained-complex-hottest-1440x1024/);
  assert.match(runnerSource, /pathname\.endsWith\('\/analyses'\)/);
  for (const anchor of ["const viewportState = () =>", "await clickSelector(`${root} .graph-counts`);"]) {
    const expression = emittedEvaluateTemplateAfter(anchor, { root: ".workspace-doctor .execution-graph" });
    assert.doesNotThrow(() => new Script(expression), `${anchor} CDP expression must compile`);
  }
  const ui = readFileSync(resolve("frontend/src/features/execution/ExecutionGraph.tsx"), "utf8");
  for (const label of ['text("Show the hottest node"', 'text("Fit all"', 'text("Full screen"', 'text("Exit full screen"']) assert.ok(ui.includes(label));
});

test("Doctor graph assertion compares the rendered canvas with backend DTO counts only", () => {
  const helper = runnerSource.slice(runnerSource.indexOf("async function assertDoctorGraph"), runnerSource.indexOf("async function doctorGraphDom"));
  assertLayoutContains(helper, "dom.card_count === graph.node_count && dom.edge_count === graph.data_flow_pair_count");
  assertLayoutContains(helper, 'const expectedFrames = dom.view !== "flow" && graph.fragment_count >= 2 ? graph.fragment_count : 0');
  assertLayoutContains(helper, "dom.table_rows === graph.summary_node_count && dom.table_all_rows === graph.node_count && dom.hot_most_count === graph.most_consuming_count && dom.hot_second_count === graph.second_most_consuming_count");
  const dom = emittedEvaluateTemplateAfter("async function doctorGraphDom", { root: ".workspace-doctor .execution-graph" });
  assert.doesNotThrow(() => new Script(dom));
  for (const selector of [".plan-card", ".react-flow__edge", ".fragment-frame", ".plan-node-table tbody tr", ".node-inspector"]) assert.ok(dom.includes(selector));
  const telemetry = runnerSource.slice(runnerSource.indexOf("const summarizeGraph"), runnerSource.indexOf("const captureProjection"));
  assert.match(telemetry, /summary\?\.state === 'available'/);
  assert.match(telemetry, /fragmentByNode\.get\(node\.node_id\) \?\? node\.fragment_id/);
  assert.doesNotMatch(runnerSource, /graph_reading|runtime_occurrence_tops|execution_graph_presentation|runtime_operator_inventory/);
});

test("fixed controlled B opens the real Overview disclosure and separately baselines route and projection telemetry", () => {
  assertLayoutContains(runnerSource, 'currentProject !== "connected-audit-docker" || transport !== "controlled_mock_over_real_session"');
  assertLayoutContains(runnerSource, "const expectedSeconds = auditWindowSecondsByPreset[auditFixtureWindow]");
  assert.match(runnerSource, /async function openAuditFixtureFilterDrawer\(\)/);
  assert.match(runnerSource, /\.audit-filter-bar button\.audit-more-filters/);
  assertLayoutContains(runnerSource, "state.drawer_present && (!requireOverviewDisclosure || state.disclosure)");
  assert.match(runnerSource, /const routesBefore = \(await routeTelemetry\(route\)\)\.length/);
  assert.match(runnerSource, /const projectionsBefore = \(await projectionTelemetry\(route\)\)\.length/);
  assert.match(runnerSource, /routes\.length === routesBefore \+ 1/);
  assertLayoutContains(runnerSource, "projectionAfterRoute(route, projectionsBefore, description)");
  assertLayoutContains(runnerSource, 'clickElement(`document.querySelector(".audit-filter-bar button[type=\'submit\']")`');
  assert.match(runnerSource, /receipt\.audit_fixture_window =/);
  assertLayoutContains(runnerSource, "const originalWindowSeconds = auditWindowSecondsByPreset[originalWindowPreset]");
  assertLayoutContains(runnerSource, "receipt.audit_filters = { ...receipt.audit_filters, original_window_preset: originalWindowPreset");
  const workload = runnerSource.slice(runnerSource.indexOf("async function auditWorkloadCoverage"), runnerSource.indexOf("async function auditFamilyDoctorCoverage"));
  // The first switch only leaves the performance overview and is asserted to
  // issue no read; family discovery is the later click that counts a new read.
  assert.ok(workload.indexOf("await auditPerformanceOverviewCoverage();") < workload.indexOf('clickButton("Repeated SQL")'));
  assertLayoutContains(workload, "Leaving the performance overview re-read the Audit overview.");
  const discovery = runnerSource.indexOf('clickButton("Repeated SQL")', runnerSource.indexOf("const familyRequestsBefore"));
  assert.ok(runnerSource.indexOf("await applyAuditFixtureWindowAfterOverview();") < discovery, "fixture window must apply before family discovery");
  // Every list discovery is an Overview read; the one exception is the selected
  // family's member ranking (design board 01 aside), a single occurrence read.
  assert.doesNotMatch(runnerSource, /"\/audit\/families"/);
  assert.equal([...runnerSource.matchAll(/routeTelemetry\("\/audit\/occurrences"\)/g)].length, 2);
});

test("official node scope selects the DTO node through the shared selection and closes its dialog", () => {
  const officialCoverage = runnerSource.slice(
    runnerSource.indexOf("async function officialProfileCoverage"),
    runnerSource.indexOf("async function identitySwitchLateReplyCoverage"),
  );
  assert.match(officialCoverage, /\.workspace-doctor \.execution-graph-canvas\[data-layout-state="ready"\]/);
  assert.match(officialCoverage, /\.workspace-doctor \.plan-node-table \.plan-node-link/);
  assert.match(officialCoverage, /\.react-flow__node\[data-id=/);
  assert.match(officialCoverage, /selected current DTO plan node/);
  assertLayoutContains(officialCoverage, "sameOfficialProfileIdentitySummary(profileIdentity.identity, identityAfterSelection?.identity)");
  assertLayoutContains(officialCoverage, "(await officialRequestLog()).length === wholeBefore + 1");
  assert.match(officialCoverage, /selected_current_dto_plan_node_preserves_identity/);
  assert.match(officialCoverage, /official_node_scope_dialog_closed_before_downstream_lifecycle/);
  assert.doesNotMatch(officialCoverage, /plan-projection|execution-locator-entry|Plan Structure/);
  const selectedIndex = officialCoverage.indexOf("selected_current_dto_plan_node_preserves_identity");
  const officialOpenIndex = officialCoverage.indexOf('await clickButton("Official reading")', selectedIndex);
  const closeDialogIndex = officialCoverage.indexOf("official node-scope dialog closed before downstream lifecycle");
  assert.ok(selectedIndex >= 0 && selectedIndex < officialOpenIndex && officialOpenIndex < closeDialogIndex, "node scope must select, read, then close in that order");
  const clickElement = runnerSource.slice(runnerSource.indexOf("async function clickElement"), runnerSource.indexOf("async function setInputValue"));
  assert.match(clickElement, /document\.elementFromPoint\(x, y\)/);
  assert.match(clickElement, /item\.contains\(hit\)/);
});

test("offline navigation uses the CDP runner and keeps Doctor-first coverage", () => {
  assert.doesNotMatch(runnerSource, /\bpage\./);
  const offline = runnerSource.slice(runnerSource.indexOf("async function offlineImportCoverage"), runnerSource.indexOf("async function retainedComplexProfileCoverage"));
  // First run (design board 05) imports from its own card; the header has no import button there.
  assert.match(offline, /setInputValue\("\.first-run \.import-panel textarea", profile\)/);
  assert.match(offline, /clickButton\("Analyze Profile"\)/);
  assert.doesNotMatch(offline, /clickButton\("Import profile"\)/);
  assert.ok(offline.includes("first_run_import_and_connect_without_demo_data"));
  assert.match(offline, /\.workspace-doctor \.doctor-summary/);
  assert.match(offline, /assertDoctorGraph\(projection, "Offline import"\)/);
  assert.ok(offline.includes(".context-bar .context-actions button"));
  assert.ok(offline.includes('summary.actions.includes("Compare before / after")'));
  assert.doesNotMatch(runnerSource, /Profile overview|workspace-overview|doctor-grid-ready/);
});

test("focused SQL browser project stays fixed-fixture only and reuses actual SQL coverage", () => {
  assert.match(runnerSource, /project === "connected-sql-docker"/);
  assert.match(runnerSource, /E2E_FIXED_DOCKER_VALIDATED === "3514-loopback"/);
  assert.match(runnerSource, /fixedSqlFixture \? "39230"/);
  assert.match(runnerSource, /await connectFixtureCoverage\(\);\s+await clickButton\("Queries"\);\s+await topologyAndRuntimeCoverage\(\);/);
  assert.doesNotMatch(runnerSource, /native-explain-analyze|nativeExplainAnalyze|NativeExplainAnalyze/);
});

test("Task49 Runtime unknown guard is opt-in, exact-session scoped, redacted, and restored", () => {
  assert.match(runnerSource, /E2E_RUNTIME_UNKNOWN_GUARD_TRANSPORT/);
  assert.match(runnerSource, /controlled_mock_over_real_session/);
  assert.match(runnerSource, /if \(runtimeUnknownGuardTransport === "controlled_mock_over_real_session"\) \{\s+await runtimeUnknownGuardCoverage\(\);/);
  assert.match(runnerSource, /if \(!normalized\) return "real_loopback_api"/);
  assert.match(runnerSource, /currentProject !== "connected-sql-docker"/);

  const coverage = runnerSource.slice(
    runnerSource.indexOf("async function runtimeUnknownGuardCoverage"),
    runnerSource.indexOf("async function currentControlledFixtureConnectionId"),
  );
  assert.match(coverage, /for \(const scenario of \["runtime_abort", "runtime_timeout"\]\)/);
  assert.match(coverage, /await connectFixtureCoverage\(\);/);
  assert.match(coverage, /await openQueryMode\("SQL workspace"\);/);
  assert.match(coverage, /connectionId !== priorConnectionId/);
  assert.match(coverage, /ordinaryExplainTimeoutDoesNotLockRuntime\(connectionId\)/);
  assert.match(coverage, /closeControlledFixtureSession\(\)/);
  assert.match(coverage, /target_forwarded_to_database === false/);
  assert.match(coverage, /task49_controlled_runtime_unknown_abort_timeout_new_sessions_no_target_sql_rerun/);

  const runtimeScenario = runnerSource.slice(
    runnerSource.indexOf("async function controlledRuntimeUnknownScenario"),
    runnerSource.indexOf("async function runtimeUnknownGuardState"),
  );
  assert.match(runtimeScenario, /await confirmStandardExecution\(\)/);
  assert.match(runtimeScenario, /stage === "unknown"/);
  assert.match(runtimeScenario, /returnedState\.execution_action_count === 0/);
  assert.match(runtimeScenario, /!document\.querySelector\('\[data-sql-analysis-action=request-confirmation\], \[data-sql-analysis-action=confirm-execution\]'\)/);
  assert.match(runtimeScenario, /clickButton\("Doctor"\)/);
  assert.match(runtimeScenario, /clickButton\("Queries"\)/);
  assert.match(runtimeScenario, /routes\.length === runtimeBefore \+ 1/);
  assert.match(runtimeScenario, /finally \{[\s\S]*?restoreRuntimeUnknownGuardMock\(\)/);

  const explainScenario = runnerSource.slice(
    runnerSource.indexOf("async function ordinaryExplainTimeoutDoesNotLockRuntime"),
    runnerSource.indexOf("async function controlledRuntimeUnknownScenario"),
  );
  assert.match(explainScenario, /installRuntimeUnknownGuardMock\(connectionId, "explain_timeout"\)/);
  assert.match(explainScenario, /data-sql-analysis-action=view-plan/);
  assert.match(explainScenario, /runtime\.disabled === false && runtime\.error === "remote_timeout"/);
  assert.match(explainScenario, /mock\?\.intercepted === 1 && mock\.target_forwarded_to_database === false/);
  assert.match(explainScenario, /result\.target_forwarded_to_database = false/);
  assert.match(explainScenario, /finally \{[\s\S]*?restoreRuntimeUnknownGuardMock\(\)/);

  const interceptor = runnerSource.slice(
    runnerSource.indexOf("async function installRuntimeUnknownGuardMock"),
    runnerSource.indexOf("async function connectedReadonlyCoverage"),
  );
  assert.match(interceptor, /url\.pathname === targetPath \+ "\/" \+ endpoint/);
  assert.match(interceptor, /if \(!isExactTarget\(input, init\)\) return previous\(input, init\)/);
  assert.match(interceptor, /new DOMException\("Task49 controlled Runtime response abort", "AbortError"\)/);
  assert.match(interceptor, /code: "remote_timeout"/);
  assert.match(interceptor, /recoverability: "restart"/);
  assert.match(interceptor, /target_forwarded_to_database: false/);
  assert.match(interceptor, /control\.restore = \(\) => \{[\s\S]*?window\.fetch = previous/);
  assert.match(interceptor, /delete window\.__doctorE2eRuntimeUnknownGuardControl/);
  const snapshot = interceptor.slice(interceptor.indexOf("control.snapshot"), interceptor.indexOf("Object.defineProperty(window, \"__doctorE2eRuntimeUnknownGuardControl\""));
  assert.doesNotMatch(snapshot, /targetPath|connectionId|candidate_sql|execution_attempt_id/);

  const controlledTelemetry = runnerSource.slice(
    runnerSource.indexOf("const recordControlledRoute"),
    runnerSource.indexOf("const hashOpaque"),
  );
  assert.match(controlledTelemetry, /normalizedRoutePath/);
  assert.match(controlledTelemetry, /runtimeSubmissions\.push\(executionAttemptIdValid\)/);
  assert.match(controlledTelemetry, /status: control\?\.status === 0 \? 0 : 504/);
  assert.doesNotMatch(controlledTelemetry, /candidate_sql|connection_id:|execution_attempt_id:/);

  for (const scenario of ["runtime_abort", "runtime_timeout", "explain_timeout"]) {
    const expression = emittedEvaluateTemplateAfter("async function installRuntimeUnknownGuardMock", {
      connectionId: "opaque-current-session",
      scenario,
    });
    assert.doesNotThrow(() => new Script(expression), `Task49 ${scenario} controlled-fetch CDP expression must compile`);
  }
});

test("standard Runtime Top official lifecycle keeps its session identity across discovery and clears the official entry on close", () => {
  const lifecycle = runnerSource.slice(
    runnerSource.indexOf("async function standardRuntimeOfficialDiscoveryLifecycleCoverage"),
    runnerSource.indexOf("async function complexPlanNavigationCoverage"),
  );
  assert.match(lifecycle, /officialProfileTransport === "real_loopback_api"/);
  assert.match(lifecycle, /await officialProfileCoverage\(\);/);
  assert.match(lifecycle, /verified_standard_attempt/);
  assert.match(lifecycle, /openQueryMode\("Audit history"\)/);
  assert.match(lifecycle, /routeTelemetry\(auditOverviewRoute\)/);
  assert.match(lifecycle, /official dialog unmounted before Queries discovery refresh/);
  assertLayoutContains(lifecycle, "sameOfficialProfileIdentitySummary(standardIdentity.identity, afterDiscoveryIdentity?.identity)");
  assert.match(lifecycle, /clickButton\("Doctor"\)/);
  assert.match(lifecycle, /assertOfficialIdentityEcho\(rereadRequest, rereadResponse\)/);
  assert.match(lifecycle, /clickSelector\("\.official-profile-return"\)/);
  assert.match(lifecycle, /official dialog closed before real session close/);
  assert.match(lifecycle, /clickButton\("Manage connection"\)/);
  assert.match(lifecycle, /clickButton\("Disconnect"\)/);
  assert.match(lifecycle, /official entry cleared after real session close/);
  assert.doesNotMatch(lifecycle, /clickButton\("Connect"\)/);
  assert.doesNotMatch(lifecycle, /installOfficialMock|configureOfficialMockQueue|releaseOfficialMock/);
  assert.ok(lifecycle.indexOf("official dialog unmounted before Queries discovery refresh") < lifecycle.indexOf("Queries Audit discovery refresh after standard Runtime Top"));
  assert.ok(lifecycle.indexOf("official dialog closed before real session close") < lifecycle.indexOf('clickButton("Manage connection")'));
  assert.match(runnerSource, /pathname\.endsWith\('\/profiles\/fetch'\) \|\| pathname\.endsWith\('\/runtime-profile'\)/);
});

test("Explain and Runtime Top graphs are checked against their DTOs, and the complex SQL stays hash-pinned", () => {
  const coverage = runnerSource.slice(runnerSource.indexOf("async function topologyAndRuntimeCoverage"), runnerSource.indexOf("async function standardRuntimeOfficialDiscoveryLifecycleCoverage"));
  assert.match(coverage, /clickButton\("Open plan graph"\)/);
  assert.match(coverage, /topologyProjection\.graph\?\.summary_state === null/);
  assert.match(coverage, /assertDoctorGraph\(\s*topologyProjection,\s*"Topology-only Explain",\s*planRoot,\s*\)/);
  assert.match(coverage, /runtimeProjection\.graph\?\.summary_state === "available"/);
  assert.match(coverage, /assertDoctorGraph\(\s*runtimeProjection,\s*"Runtime Top",\s*\)/);
  assert.match(coverage, /\.explain-plan-overlay-header button/);
  assert.match(runnerSource, /517b9da3b06e8c7e41864b6187793d46267f110b1c4b5b21b12b555b8eeef024/);
  const complex = runnerSource.slice(runnerSource.indexOf("async function complexPlanNavigationCoverage"), runnerSource.indexOf("async function runtimeTopNavigationCoverage"));
  assert.match(complex, /graph\.node_count > 12 && graph\.fragment_count >= 2/);
  assert.match(complex, /clickButton\("Full screen"\)/);
  assert.match(complex, /Plan estimates/);
});

test("Task53 confirmation preserves SQL and performs no request until confirmation", () => {
  const start = runnerSource.indexOf("async function confirmStandardExecution(");
  const end = runnerSource.indexOf("async function installRuntimeUnknownGuardMock", start);
  assert.ok(start >= 0 && end > start, "Confirmation helper boundaries must exist.");
  const body = runnerSource.slice(start, end);
  assert.match(body, /const paths = \["\/explain", "\/runtime-profile"\];/);
  assert.match(body, /Confirmation changed the exact SQL draft/);
  assert.match(body, /Review step executed SQL before explicit confirmation/);
  const noRequestCheck = body.search(/assert\(\s*after\.every/);
  const dispatch = body.search(/if\s*\(\s*dispatch\s*\)/);
  assert.ok(noRequestCheck >= 0 && dispatch >= 0, "Both request guard and dispatch must exist.");
  assert.ok(noRequestCheck < dispatch, "Request guard must precede dispatch.");
  assert.match(runnerSource, /await confirmStandardExecution\(false\);/);
  const expression = emittedEvaluateTemplateAfter("async function runtimeUnknownGuardState()", {});
  assert.doesNotThrow(() => new Script(expression));
  assert.match(expression, /execution_action_count/);
  assert.doesNotMatch(runnerSource, /candidate-plan-sql|sql-analysis-mode button|Execute & analyze Runtime Top/);
});

test("readonly discovers bounded retained Profiles before Audit self-observation", () => {
  const source = runnerSource.slice(runnerSource.indexOf("async function connectedReadonlyCoverage()"), runnerSource.indexOf("async function sqlDraftRoundTripCoverage"));
  assert.ok(source.indexOf('await openQueryMode("Profile list")') >= 0);
  assert.ok(source.indexOf('await openQueryMode("Profile list")') < source.indexOf('await clickButton("Refresh")'));
  assert.ok(source.indexOf('await clickButton("Compare selected Profiles")') < source.indexOf('await auditWorkloadCoverage()'));
  assert.equal((source.match(/await auditWorkloadCoverage\(\)/g) ?? []).length, 1);
  assert.ok(source.indexOf('await auditWorkloadCoverage()') < source.indexOf('await topologyAndRuntimeCoverage()'));
  assert.ok(source.lastIndexOf('await clickButton("Compare")') > source.indexOf('await topologyAndRuntimeCoverage()'));
  const product = readFileSync(resolve("frontend/src/features/queries/QueriesWorkspace.tsx"), "utf8");
  assert.match(product, /limit: 100/);
  assert.equal((source.match(/await clickButton\("Refresh"\)/g) ?? []).length, 1);
});

test("Runtime Top table row reveals the backend hottest node inside canvas and page", () => {
  const coverage = runnerSource.slice(runnerSource.indexOf("async function runtimeTopNavigationCoverage"), runnerSource.indexOf("async function narrowViewportEvidence"));
  assert.match(coverage, /\.plan-node-table tbody tr:first-child/);
  assert.match(coverage, /hottest\.display && \(await textOf\(firstRow\)\)\.includes\(hottest\.display\)/);
  assert.match(coverage, /\.inspector-operators tbody tr/);
  assert.doesNotMatch(coverage, /(?:scrollIntoView|scrollTo|setCenter)\s*\(|await wait\(/, "test must not perform the product scroll or add a fixed delay");
  const expression = emittedEvaluateTemplateAfter("await clickSelector(`${firstRow} .plan-node-link`);", { root: ".workspace-doctor .execution-graph", hottest: { node_id: "0" } });
  const script = new Script(expression);
  const canvas = { left: 10, right: 800, top: 0, bottom: 500 };
  let node = { width: 200, height: 120, left: 200, right: 400, top: 900, bottom: 1020 };
  const card = { dataset: { nodeId: "0" }, getBoundingClientRect: () => node };
  const root = { querySelector: (selector) => selector === ".plan-card.selected" ? card : selector === ".execution-graph-canvas" ? { getBoundingClientRect: () => canvas } : selector === ".node-inspector" ? {} : null };
  const context = { innerWidth: 1440, innerHeight: 1024, Math, JSON, document: { querySelector: () => root } };
  assert.equal(script.runInNewContext(context), false, "selected node outside the canvas is not visible");
  node = { ...node, top: 180, bottom: 300 };
  assert.equal(script.runInNewContext(context), true, "same predicate accepts a centered node");
  for (const patch of [{ left: -1 }, { right: 801 }, { top: -1 }, { bottom: 501 }, { width: 0 }, { height: 0 }]) {
    const original = node; node = { ...node, ...patch };
    assert.equal(script.runInNewContext(context), false);
    node = original;
  }
  card.dataset.nodeId = "1";
  assert.equal(script.runInNewContext(context), false, "a different selected node is rejected");
});

test("current user-facing connection and Audit copy preserves semantic and DTO checks", () => {
  const connectionUi = readFileSync(resolve("frontend/src/features/connection/ConnectionDialog.tsx"), "utf8");
  const auditUi = readFileSync(resolve("frontend/src/features/audit/AuditWorkloadWorkspace.tsx"), "utf8");
  const doctorUi = readFileSync(resolve("frontend/src/features/doctor/DoctorWorkspace.tsx"), "utf8");
  assert.ok(connectionUi.includes('text("Connected",'));
  assert.ok(runnerSource.includes('connectionState.startsWith("Connected") && connectionState.includes("3.5.14")'));
  const draft = "Filters changed. Apply them to update the results.";
  assert.ok(auditUi.includes(draft)); assert.ok(runnerSource.includes(draft));
  const appUi = readFileSync(resolve("frontend/src/App.tsx"), "utf8");
  assert.ok(doctorUi.includes('text("Total time"')); assert.ok(runnerSource.includes("Total time"));
  assert.ok(appUi.includes('text("Compare before / after"')); assert.ok(runnerSource.includes("Compare before / after"));
  assert.ok(appUi.includes("data-count={item === \"Recommendations\"")); assert.ok(runnerSource.includes("recommendationsCount"));
  assert.ok(!runnerSource.includes('text.includes("Pending draft")'));
  assert.ok(runnerSource.includes('JSON.stringify(unapplied.audit_scope) ==='));
  assert.ok(runnerSource.includes('applied.audit_scope?.window_seconds === 2592000'));
  assert.ok(runnerSource.includes('typeof overviewCoverage.missing_digest_count === "string"'));
});

test("Queries modes are entered explicitly and SQL workspace opens without executing", () => {
  const helper = runnerSource.slice(runnerSource.indexOf("async function openQueryMode"), runnerSource.indexOf("async function navigate"));
  assert.match(helper, /\.workspace-queries \.query-modes \[role="tab"\]/);
  assert.match(helper, /getAttribute\('aria-selected'\) === 'true'/);
  const block = runnerSource.slice(runnerSource.indexOf("async function topologyAndRuntimeCoverage()"));
  const entry = block.slice(0, block.indexOf("const explainRoutesBefore"));
  assert.ok(entry.includes('await openQueryMode("SQL workspace")'));
  assert.ok(entry.includes('enteredCounts.every((count, index) => count === entryCounts[index])'));
  const app = readFileSync(resolve("frontend/src/App.tsx"), "utf8");
  for (const label of ["Audit history", "Profile list", "Running now", "SQL workspace"]) assert.ok(app.includes(`text("${label}"`), `query mode ${label} missing`);
  assert.match(runnerSource, /openQueryMode\("Audit history"\)/);
});

test("Audit individual-execution table fits beside the selection panel at 1440 wide", () => {
  const css = readFileSync(resolve("frontend/src/features/audit/audit-workload.css"), "utf8");
  const auditUi = readFileSync(resolve("frontend/src/features/audit/AuditWorkloadWorkspace.tsx"), "utf8");
  // 1440 - page scrollbar - shell padding - 12px gap - 400px detail panel - card border, as measured
  // in the release E2E with real audit rows (956px; 986px without a page scrollbar).
  const budget = 956;
  // SQL (column 2) has no width: it takes what the other columns leave, and must keep a readable prefix.
  // With the detail panel open, the three metrics the panel repeats (6, 7, 10) leave the table, and a
  // fixed minimum width makes the table scroll inside its card instead of clipping a number's unit.
  const sqlMinimum = 150;
  const fullMinimum = 1100;
  const besidePanelMinimum = 824;
  const repeatedInPanel = [6, 7, 10];
  assert.ok(css.includes(".audit-table-wrap { min-height: 180px; max-height: 560px; overflow: auto; }"));
  assert.ok(css.includes(".audit-results-layout { display: grid; grid-template-columns: minmax(0, 1fr) 360px;"));
  assert.ok(css.includes(`.audit-table-wrap table.audit-occurrence-table { min-width: ${fullMinimum}px; table-layout: fixed; }`));
  assert.ok(css.includes(`.audit-board-layout:has(> .audit-selection) table.audit-occurrence-table { min-width: ${besidePanelMinimum}px; }`));
  assert.ok(css.includes(".audit-board-layout:has(> .audit-selection) .audit-occurrence-table :is(th, td):is(:nth-child(6), :nth-child(7), :nth-child(10)) { display: none; }"));
  assert.ok(css.includes(".audit-occurrence-table td.audit-cell-num { text-overflow: clip; }"));
  assert.ok(auditUi.includes('className={activeAt ? undefined : "audit-occurrence-table"}'));
  const widths = new Map();
  for (const [, selectors, value, unit] of css.matchAll(/^(\.audit-occurrence-table th:nth-child[^{]+)\{ width: ([\d.]+)(px|%); \}$/gm)) {
    for (const selector of selectors.split(",")) {
      const range = selector.match(/nth-child\(n\+(\d+)\):nth-child\(-n\+(\d+)\)/);
      const columns = range ? Array.from({ length: Number(range[2]) - Number(range[1]) + 1 }, (_, index) => Number(range[1]) + index) : [Number(selector.match(/nth-child\((\d+)\)/)[1])];
      for (const column of columns) widths.set(column, unit === "px" ? Number(value) : Number(value) / 100 * budget);
    }
  }
  assert.deepEqual([...widths.keys()].sort((a, b) => a - b), [1, 3, 4, 5, 6, 7, 8, 9, 10, 11], "every column but SQL has a width");
  assert.ok([...widths.entries()].every(([, width]) => Number.isFinite(width)), "every width is a fixed px value");
  const total = [...widths.values()].reduce((sum, width) => sum + width, 0);
  const besidePanel = [...widths.entries()].filter(([column]) => !repeatedInPanel.includes(column)).reduce((sum, [, width]) => sum + width, 0);
  assert.ok(total + sqlMinimum <= fullMinimum, `columns need ${total}px, leaving SQL less than ${sqlMinimum}px of the ${fullMinimum}px table`);
  assert.ok(besidePanel + sqlMinimum <= besidePanelMinimum, `columns beside the panel need ${besidePanel}px, leaving SQL less than ${sqlMinimum}px`);
  assert.ok(besidePanelMinimum <= budget, `the table beside the panel needs ${besidePanelMinimum}px of the ${budget}px at 1440 wide`);
});
