#!/usr/bin/env node
// Reads every Profile captured by run-version-smoke.ps1 through a running
// doctor-api and summarizes what the product made of it, per release:
//
//   node scripts/version-audit/check-version-smoke.mjs --api http://127.0.0.1:18092 [--root fixtures/runtime/version-smoke]
//
// The summary (version-smoke-summary.json next to the captures) is the
// receipt's evidence: version resolution, metric typing, graph, time share,
// findings and diagnostics. A capture the product cannot resolve fails.
import { readdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { join, resolve } from "node:path";

export function parseArgs(argv) {
  const args = { api: null, root: "fixtures/runtime/version-smoke" };
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (value === undefined) throw new Error(`${flag} needs a value`);
    if (flag === "--api") args.api = value;
    else if (flag === "--root") args.root = value;
    else throw new Error(`unknown option ${flag}`);
    index += 1;
  }
  if (!args.api) throw new Error("usage: --api <doctor-api base url> [--root <captures>]");
  return args;
}

/** The facts the receipt needs from one analysis report. */
export function summarizeReport(report) {
  const diagnostics = {};
  for (const diagnostic of report.diagnostics ?? []) diagnostics[diagnostic.code] = (diagnostics[diagnostic.code] ?? 0) + 1;
  const pack = report.version.selected_pack;
  return {
    effective_version: report.version.effective_version,
    pack_match: report.version.pack_match,
    support_status: report.version.support_status,
    compatibility: report.version.compatibility,
    delta_ids: pack?.release_deltas?.delta_ids ?? [],
    issues: report.version.issues,
    profile_limitations: report.profile_context.limitations,
    metrics_completeness: report.completeness.metrics.state,
    graph_state: report.graph_view.state,
    graph_nodes: report.graph_view.nodes.length,
    graph_edges: report.graph_view.edges.length,
    execution_summary: report.execution_summary?.state ?? null,
    time_shared_nodes: (report.execution_summary?.nodes ?? []).filter((node) => node.time_share_display).length,
    findings: (report.findings ?? []).map((finding) => `${finding.rule_id}:${finding.severity}`),
    diagnostics,
  };
}

/**
 * Per plan node from the official `ANALYZE PROFILE` text (ExplainAnalyzer,
 * 3.1+): its time share and output rows. Colour codes are stripped; a count
 * printed as "73.728K (73728)" reads the bracketed raw value.
 */
export function parseAnalyze(text) {
  const nodes = new Map();
  let current = null;
  for (const raw of text.replace(/\u001b\[[0-9;]*m/g, "").split(/\r?\n/)) {
    const header = /(?:└──|├──)\s*(?:<[A-Z]+>\s*)?(?:<-)?\[?([A-Z_]+)\]?\s*\(id=(\d+)\)/.exec(raw);
    // A fragment sink carries the id of the exchange it feeds; the exchange is the plan node.
    if (header && header[1].endsWith("_SINK")) { current = null; continue; }
    if (header) {
      current = { type: header[1], share: null, rows: null };
      nodes.set(header[2], current);
      continue;
    }
    if (/(?:└──|├──)/.test(raw)) { current = null; continue; }
    if (!current) continue;
    const time = /TotalTime: [^(]*\(([\d.]+)%\)/.exec(raw);
    if (time && current.share === null) current.share = Number(time[1]);
    const rows = /OutputRows: ([\d.]+[KMB]?)(?: \((\d+)\))?/.exec(raw);
    if (rows && current.rows === null) current.rows = rows[2] ?? (/^\d+$/.test(rows[1]) ? rows[1] : null);
  }
  return nodes;
}

/** Our execution summary against the official analysis: shares within 0.01 points, rows exact. */
export function compareWithOfficial(report, official) {
  const mismatches = [];
  let compared = 0;
  for (const node of report.execution_summary?.nodes ?? []) {
    const reference = official.get(node.plan_node_id ?? node.node_id);
    if (!reference) continue;
    compared += 1;
    const share = node.time_share_display ? Number(node.time_share_display.replace("%", "")) : null;
    if (reference.share !== null && (share === null || Math.abs(share - reference.share) > 0.0101)) {
      mismatches.push(`share:${node.node_id}:${share}!=${reference.share}`);
    }
    const rows = node.output_rows?.canonical_number?.value ?? null;
    if (reference.rows !== null && rows !== reference.rows) mismatches.push(`rows:${node.node_id}:${rows}!=${reference.rows}`);
  }
  return { compared, mismatches };
}

/** A capture passes when its release resolves to a pack and the graph and time share are readable. */
export function verdict(summary) {
  const problems = [];
  if (!["exact_pack", "baseline_with_deltas"].includes(summary.pack_match)) problems.push(`pack_match=${summary.pack_match}`);
  if (summary.support_status !== "supported") problems.push(`support_status=${summary.support_status}`);
  if (summary.graph_state === "unavailable" || summary.graph_nodes === 0) problems.push("graph_unavailable");
  if (summary.execution_summary !== "available") problems.push(`execution_summary=${summary.execution_summary}`);
  const parseFailures = Object.entries(summary.diagnostics).filter(([code]) => /invalid|overflow|unit|parse_failed/.test(code));
  if (parseFailures.length) problems.push(`parse:${parseFailures.map(([code, count]) => `${code}x${count}`).join(",")}`);
  if (summary.official?.mismatches.length) problems.push(...summary.official.mismatches.map((mismatch) => `official:${mismatch}`));
  return problems;
}

async function analyze(api, nonce, profile) {
  const form = new FormData();
  form.append("profile", new Blob([profile], { type: "text/plain" }), "profile.txt");
  form.append("source_kind", "text_profile");
  const response = await fetch(`${api}/api/v1/analyses`, { method: "POST", headers: { Origin: api, "x-doctor-csrf": nonce }, body: form });
  const body = await response.json();
  if (!response.ok) throw new Error(`analysis failed: ${response.status} ${body.code ?? ""}`);
  return body.outcome.report;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const capabilities = await (await fetch(`${args.api}/api/v1/capabilities`)).json();
  const nonce = capabilities.csrf.nonce;
  const root = resolve(args.root);
  const releases = readdirSync(root, { withFileTypes: true }).filter((entry) => entry.isDirectory()).map((entry) => entry.name).sort();
  const summary = {};
  let failed = 0;
  for (const release of releases) {
    const folder = join(root, release);
    const capture = existsSync(join(folder, "capture.json")) ? JSON.parse(readFileSync(join(folder, "capture.json"), "utf8")) : null;
    summary[release] = { capture_status: capture?.status ?? "missing", image_digest: capture?.image_digest ?? null, profiles: {} };
    for (const file of readdirSync(folder).filter((name) => name.endsWith(".profile.txt")).sort()) {
      const report = await analyze(args.api, nonce, readFileSync(join(folder, file), "utf8"));
      const result = summarizeReport(report);
      const analyzeFile = join(folder, file.replace(".profile.txt", ".analyze.txt"));
      if (existsSync(analyzeFile)) result.official = compareWithOfficial(report, parseAnalyze(readFileSync(analyzeFile, "utf8")));
      result.problems = verdict(result);
      if (result.problems.length) failed += 1;
      summary[release].profiles[file] = result;
      process.stdout.write(`${release} ${file}: ${result.problems.length ? `FAIL ${result.problems.join(" ")}` : "ok"} (${result.pack_match}, ${result.graph_nodes} nodes, ${result.findings.length} findings${result.official ? `, official ${result.official.compared} nodes` : ""})\n`);
    }
  }
  writeFileSync(join(root, "version-smoke-summary.json"), `${JSON.stringify(summary, null, 2)}\n`);
  process.exitCode = failed ? 1 : 0;
}

if (process.argv[1] && resolve(process.argv[1]).endsWith("check-version-smoke.mjs")) await main();
