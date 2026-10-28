import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { createRequire } from "node:module";

const [, , referencePath, actualPath, diffPath, reportPath] = process.argv;
if (!referencePath || !actualPath || !diffPath || !reportPath) {
  throw new Error("usage: node visual-compare.mjs <reference.png> <actual.png> <diff.png> <report.json>");
}

const dependencyRoot = process.env.CODEX_WORKSPACE_NODE_MODULES;
const require = dependencyRoot
  ? createRequire(path.join(dependencyRoot, "package.json"))
  : createRequire(import.meta.url);
const { PNG } = require("pngjs");
const pixelmatchModule = require("pixelmatch");
const pixelmatch = pixelmatchModule.default ?? pixelmatchModule;
const pixelmatchVersion = require("pixelmatch/package.json").version;

const reference = PNG.sync.read(fs.readFileSync(referencePath));
const actual = PNG.sync.read(fs.readFileSync(actualPath));
if (reference.width !== actual.width || reference.height !== actual.height) {
  throw new Error(`viewport mismatch: reference ${reference.width}x${reference.height}, actual ${actual.width}x${actual.height}`);
}

const diff = new PNG({ width: reference.width, height: reference.height });
const mismatchedPixels = pixelmatch(reference.data, actual.data, diff.data, reference.width, reference.height, {
  threshold: 0.1,
  includeAA: false,
});
const totalPixels = reference.width * reference.height;
const mismatchRatio = mismatchedPixels / totalPixels;
const report = {
  comparator: "pixelmatch",
  comparator_version: pixelmatchVersion,
  reference: path.resolve(referencePath),
  actual: path.resolve(actualPath),
  reference_sha256: crypto.createHash("sha256").update(fs.readFileSync(referencePath)).digest("hex"),
  actual_sha256: crypto.createHash("sha256").update(fs.readFileSync(actualPath)).digest("hex"),
  viewport: { width: reference.width, height: reference.height },
  threshold: 0.1,
  antialiasing_ignored: true,
  masks: [],
  mismatched_pixels: mismatchedPixels,
  total_pixels: totalPixels,
  mismatch_ratio: Number(mismatchRatio.toFixed(6)),
  similarity_ratio: Number((1 - mismatchRatio).toFixed(6)),
  gate_90_percent: mismatchRatio <= 0.1 ? "PASS" : "FAIL",
  command_exit_code: 0,
  generated_at_utc: new Date().toISOString(),
};

fs.mkdirSync(path.dirname(diffPath), { recursive: true });
fs.mkdirSync(path.dirname(reportPath), { recursive: true });
fs.writeFileSync(diffPath, PNG.sync.write(diff));
fs.writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify(report));
