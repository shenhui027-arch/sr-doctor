import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { AnalysisSnapshotReport, ConnectedQueryRef, ConnectionSessionView, OfficialProfileReadResponse, ProfileFetchResponse } from "../../../../contracts/generated/doctor-contracts";
import { sr3514ConnectedContractGolden } from "../../../../contracts/generated/connected-contract-fixtures";
import { LocaleProvider } from "../../i18n/locale";
import { SelectionProvider } from "../selection/selection-store";
import { officialBindingForScope, officialScopeSwitchLocked, OfficialProfilePanel, OfficialProfileReadDetails, type OfficialDisplayState, type OfficialProfileContext } from "./OfficialProfilePanel";
import type { OfficialBinding, OfficialReadState } from "./official-profile-read";

const binding: OfficialBinding = Object.freeze({
  connection_id: "connection-1",
  query_id: "query-1",
  identity: { kind: "discovery" as const, discovery_generation: 7 },
  node_ids: Object.freeze([]),
});

function response(): OfficialProfileReadResponse {
  return {
    kind: "analyze_profile_text", identity: binding.identity,
    query: { ...sr3514ConnectedContractGolden.query_response.queries[0], connection_id: binding.connection_id, query_id: binding.query_id, discovery_generation: (binding.identity.kind === "discovery" ? binding.identity.discovery_generation : null) },
    detected_version: { value: "3.5.14", source_ref: "connection:version" },
    node_ids: [],
    read_at_utc: "2026-09-06T07:30:00Z",
    profile_observed_at_utc: null,
    source_hash: "raw-sha",
    text: "\u001b[31m<script data-unsafe=\"1\">raw</script>",
    display_text: "<script data-unsafe=\"1\">display</script>\nfull display text",
    display_hash: "display-sha",
    display_normalization: "ansi_sgr_strip_controls_escape_v1",
    source_refs: ["profile:line:1", "profile:line:2"],
    limitations: ["profile_observed_at_unknown"],
  };
}

function context(report: AnalysisSnapshotReport): OfficialProfileContext {
  const session: ConnectionSessionView = { ...sr3514ConnectedContractGolden.connection, connection_id: binding.connection_id, state: "ready" };
  const query: ConnectedQueryRef = { ...sr3514ConnectedContractGolden.profile_fetch_response.query, connection_id: binding.connection_id, query_id: binding.query_id, discovery_generation: (binding.identity.kind === "discovery" ? binding.identity.discovery_generation : null) };
  const evidence: ProfileFetchResponse = { ...sr3514ConnectedContractGolden.profile_fetch_response, query, input_origin: "connected_profile_fetch", state: "fetched", outcome: { report } };
  return { session, currentQuery: query, evidence };
}

function render(node: ReturnType<typeof createElement>): string {
  return renderToStaticMarkup(createElement(LocaleProvider, { initialLocale: "en", children: node }));
}

describe("OfficialProfilePanel presentation", () => {
  it("renders source and display artifacts as React text without hash identifiers", () => {
    const state: OfficialDisplayState = { status: "success", binding, response: response() };
    const html = render(<OfficialProfileReadDetails state={state} />);
    expect(html).toContain("Official ANALYZE PROFILE text");
    expect(html).not.toContain("raw-sha");
    expect(html).not.toContain("display-sha");
    expect(html).toContain("ansi_sgr_strip_controls_escape_v1");
    expect(html).toContain("2026-09-06T07:30:00Z");
    expect(html).toContain("Source did not provide");
    expect(html).toContain("profile:line:1");
    expect(html).toContain("profile:line:2");
    expect(html).toContain("profile_observed_at_unknown");
    expect(html).toContain("official-profile-display-text");
    expect(html).toContain("official-profile-raw-source");
    expect(html).toContain("&lt;script data-unsafe=&quot;1&quot;&gt;display&lt;/script&gt;");
    expect(html).not.toContain("<script data-unsafe");
  });

  it("keeps typed unavailable distinct from a successful artifact", () => {
    const state: OfficialDisplayState = {
      status: "unavailable",
      binding,
      error: { code: "official_profile_unavailable", stage: "analyze_profile", message_id: "official_profile_empty", recoverability: "change_input", details: { limitation: "profile_empty" } },
    };
    const html = render(<OfficialProfileReadDetails state={state} />);
    expect(html).toContain("Official analysis is unavailable for this range.");
    expect(html).toContain("StarRocks returned an empty analysis for this Profile.");
    // The raw ids stay available, folded into technical details.
    expect(html).toContain('<details class="official-profile-technical">');
    expect(html).toContain("official_profile_empty");
    expect(html).toContain("change_input");
    expect(html).not.toContain("official-profile-display-text");
    expect(html).not.toContain("raw-sha");
  });

  it("explains an evicted Profile in readable words (v0.6-10)", () => {
    const state: OfficialDisplayState = {
      status: "unavailable",
      binding,
      error: { code: "official_profile_unavailable", stage: "analyze_profile", message_id: "official_profile_not_retained", recoverability: "change_input", details: {} },
    };
    const html = render(<OfficialProfileReadDetails state={state} />);
    expect(html).toContain("StarRocks no longer keeps this Profile (it was evicted or expired).");
    expect(html).not.toContain("Official analysis request failed.");
  });

  it("does not coerce a missing or lost selection into an entire-Query node binding", () => {
    const report: AnalysisSnapshotReport = structuredClone(sr3514ConnectedContractGolden.profile_fetch_response.outcome.report);
    const accepted = context(report);
    expect(officialBindingForScope(accepted, report, "plan_node", null)).toBeNull();
    const node = { ...report.graph_view.nodes[0], node_id: "plan:27", entity_kind: "plan_node" as const, plan_node_id: "27" };
    report.graph_view.nodes = [node];
    expect(officialBindingForScope(accepted, report, "plan_node", node)?.node_ids).toEqual([27]);
    expect(officialBindingForScope(accepted, report, "plan_node", null)).toBeNull();
  });

  it("locks scope after whole-range and restart blocks, but lets node unavailable return to whole", () => {
    const wholeUnavailable: OfficialReadState = { status: "unavailable", binding,
      error: { code: "official_profile_unavailable", stage: "analyze_profile", message_id: "official_profile_empty", recoverability: "change_input", details: {} } };
    const nodeUnavailable: OfficialReadState = { ...wholeUnavailable, binding: { ...binding, node_ids: [27] } };
    const restart: OfficialReadState = { ...nodeUnavailable,
      error: { code: "remote_timeout", stage: "analyze_profile", message_id: "remote_timeout", recoverability: "restart", details: {} } };
    const expired: OfficialReadState = { ...nodeUnavailable,
      error: { code: "profile_unavailable", stage: "analyze_profile", message_id: "profile_unavailable", recoverability: "change_input", details: { limitation: "query_discovery_generation_expired" } } };
    expect(officialScopeSwitchLocked(wholeUnavailable)).toBe(true);
    expect(officialScopeSwitchLocked(nodeUnavailable)).toBe(false);
    expect(officialScopeSwitchLocked(restart)).toBe(true);
    expect(officialScopeSwitchLocked(expired)).toBe(true);
  });
  it("shows a read entry only for the same current fetched Doctor evidence", () => {
    const report: AnalysisSnapshotReport = structuredClone(sr3514ConnectedContractGolden.profile_fetch_response.outcome.report);
    const accepted = context(report);
    const expected = render(<SelectionProvider><OfficialProfilePanel context={accepted} report={report} /></SelectionProvider>);
    const stale = render(<SelectionProvider><OfficialProfilePanel context={accepted} report={{ ...report }} /></SelectionProvider>);
    expect(expected).toContain("Official reading");
    expect(expected).toContain('aria-haspopup="dialog"');
    expect(stale).toBe("");
  });
});