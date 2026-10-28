import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "../..");
const rootHtml = readFileSync(resolve(root, "index.html"), "utf8");

for (const relativePath of [
  ".claude",
  "css",
  "js",
  "northstar_dark.png",
  "northstar_light.png",
  "design/references/northstar",
  "docs/v0.1/prototypes/references/northstar-empty-1440x1024.png",
  "artifacts/visual-qa/global-design-audit/freeze-20260829-no-ai/northstar-vs-current-overview.png",
]) {
  assert.equal(existsSync(resolve(root, relativePath)), false, `legacy reference path must be absent: ${relativePath}`);
}

for (const [label, pattern] of [
  ["script element", /<script\b/i],
  ["external URL", /(?:https?:)?\/\//i],
  ["asset link", /<(?:link|img|iframe|object|embed)\b/i],
  ["asset attribute", /\b(?:src|href)\s*=\s*["']/i],
  ["upload control", /<(?:form|input|textarea|button)\b/i],
  ["network/runtime API", /\b(?:fetch|XMLHttpRequest|sendBeacon|gtag|goatcounter|analytics|telemetry)\b/i],
  ["automatic redirect", /(?:http-equiv\s*=\s*["']refresh|window\.location|location\.(?:assign|replace))\b/i],
]) {
  assert(!pattern.test(rootHtml), `root index contains ${label}`);
}

for (const productPath of ["frontend", "backend", "contracts"]) {
  assert.equal(existsSync(resolve(root, productPath)), true, `product path missing: ${productPath}`);
}

process.stdout.write("root-entry PASS: legacy reference source/assets absent; static root isolated\n");
