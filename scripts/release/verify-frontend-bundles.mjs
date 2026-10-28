import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, relative, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";

export const LIMITS = Object.freeze({
  entry: 300_000,
  // v0.2 audit charts add at most 25 KB over the original app budget. Count
  // every app chunk so route splitting cannot conceal cumulative growth.
  applicationJavaScript: 325_000,
  reactVendor: 250_000,
  graphVendor: 250_000,
  graphWorker: 500_000,
  elkWorker: 1_700_000,
  graphPath: 1_983_436,
  otherJavaScript: 500_000,
});

const SCHEMA_VERSION = "sr-doctor.frontend-bundles/v0.1";

function fail(code, message, details = {}) {
  const error = new Error(message);
  error.code = code;
  error.details = details;
  throw error;
}

function pathInside(root, target) {
  const fromRoot = relative(root, target);
  return fromRoot !== "" && fromRoot !== ".." && !fromRoot.startsWith(`..${sep}`) && !isAbsolute(fromRoot);
}

function relativeAssetPath(root, target) {
  return relative(root, target).split(sep).join("/");
}

function listFiles(root) {
  const files = [];
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const path = resolve(root, entry.name);
    if (entry.isDirectory()) files.push(...listFiles(path));
    else if (entry.isFile()) files.push(path);
  }
  return files;
}

function assetRecord(dist, path) {
  return {
    path: relativeAssetPath(dist, path),
    name: path.slice(path.lastIndexOf(sep) + 1),
    size_bytes: statSync(path).size,
    source: readFileSync(path, "utf8"),
  };
}

function resolveScriptReference(dist, reference) {
  if (/^(?:[a-z]+:)?\/\//i.test(reference) || /^(?:data|javascript):/i.test(reference)) {
    fail("entry_external_reference", "index.html contains an external script reference");
  }
  const withoutQuery = reference.split(/[?#]/, 1)[0];
  const relativeReference = withoutQuery.replace(/^\//, "").replaceAll("\\", "/");
  const path = resolve(dist, relativeReference);
  if (!pathInside(dist, path) || !existsSync(path) || !statSync(path).isFile()) {
    fail("entry_missing", "index.html references a missing JavaScript asset", { reference });
  }
  return path;
}

function oneNamed(records, pattern, label) {
  const matches = records.filter(({ name }) => pattern.test(name));
  if (matches.length !== 1) {
    fail(`${label}_count`, `expected exactly one ${label} asset`, {
      matches: matches.map(({ path }) => path),
    });
  }
  return matches[0];
}

function assertLimit(record, limit, code, label, onExceeded = fail) {
  if (record.size_bytes > limit) {
    onExceeded(code, `${label} exceeds ${limit} bytes`, {
      path: record.path,
      size_bytes: record.size_bytes,
      limit_bytes: limit,
    });
  }
}

function assertReference(source, target, code, label) {
  if (!source.source.includes(target.name)) {
    fail(code, `${label} does not reference ${target.name}`, {
      source: source.path,
      target: target.path,
    });
  }
}

export function verifyFrontendBundles(distPath, policy = "strict-v0.1") {
  if (!["strict-v0.1", "private-v0.2-size-observation"].includes(policy)) fail("invalid_policy", "unknown bundle policy");
  const warnings = [];
  const sizeExceeded = (code, message, details) => {
    if (policy === "strict-v0.1") fail(code, message, details);
    warnings.push({ code, message, ...details });
  };
  const checkSize = (record, limit, code, label) => assertLimit(record, limit, code, label, sizeExceeded);
  const dist = resolve(distPath);
  const indexPath = resolve(dist, "index.html");
  const assetsRoot = resolve(dist, "assets");
  if (!existsSync(indexPath) || !statSync(indexPath).isFile()) fail("index_missing", "dist/index.html is missing");
  if (!existsSync(assetsRoot) || !statSync(assetsRoot).isDirectory()) fail("assets_missing", "dist/assets is missing");

  const html = readFileSync(indexPath, "utf8");
  const scriptReferences = [...html.matchAll(/<script\b[^>]*\bsrc=["']([^"']+\.js(?:[?#][^"']*)?)["'][^>]*>/gi)].map((match) => match[1]);
  if (scriptReferences.length !== 1) {
    fail("entry_count", "index.html must reference exactly one JavaScript entry", { references: scriptReferences });
  }
  const entryPath = resolveScriptReference(dist, scriptReferences[0]);
  if (!pathInside(assetsRoot, entryPath)) fail("entry_not_in_assets", "JavaScript entry must live under dist/assets");

  const javascript = listFiles(assetsRoot)
    .filter((path) => path.toLowerCase().endsWith(".js"))
    .map((path) => assetRecord(dist, path));
  const entry = javascript.find(({ path }) => resolve(dist, path) === entryPath);
  if (!entry) fail("entry_not_in_assets", "JavaScript entry is not an emitted asset", { path: relativeAssetPath(dist, entryPath) });

  const reactVendor = oneNamed(javascript, /^react-vendor-[A-Za-z0-9_-]+\.js$/, "react-vendor");
  const graphVendor = oneNamed(javascript, /^graph-vendor-[A-Za-z0-9_-]+\.js$/, "graph-vendor");
  const graphWorker = oneNamed(javascript, /^graph-layout\.worker-[A-Za-z0-9_-]+\.js$/, "graph-layout.worker");
  const elkWorker = oneNamed(javascript, /^elk-worker\.min-[A-Za-z0-9_-]+\.js$/, "elk-worker.min");

  checkSize(entry, LIMITS.entry, "entry_too_large", "entry");
  checkSize(reactVendor, LIMITS.reactVendor, "react_vendor_too_large", "react-vendor");
  checkSize(graphVendor, LIMITS.graphVendor, "graph_vendor_too_large", "graph-vendor");
  checkSize(graphWorker, LIMITS.graphWorker, "graph_worker_too_large", "graph-layout.worker");
  checkSize(elkWorker, LIMITS.elkWorker, "elk_worker_too_large", "elk-worker.min");
  const graphPathBytes = graphWorker.size_bytes + elkWorker.size_bytes;
  if (graphPathBytes > LIMITS.graphPath) {
    sizeExceeded("graph_path_too_large", "graph worker plus ELK asset exceeds the graph-path budget", {
      graph_worker_bytes: graphWorker.size_bytes,
      elk_worker_bytes: elkWorker.size_bytes,
      total_bytes: graphPathBytes,
      limit_bytes: LIMITS.graphPath,
    });
  }
  for (const asset of javascript) {
    if (asset === elkWorker) continue;
    checkSize(asset, LIMITS.otherJavaScript, "javascript_asset_too_large", "JavaScript asset");
  }
  const applicationJavaScriptBytes = javascript
    .filter((asset) => ![reactVendor, graphVendor, graphWorker, elkWorker].includes(asset))
    .reduce((sum, asset) => sum + asset.size_bytes, 0);
  if (applicationJavaScriptBytes > LIMITS.applicationJavaScript) {
    sizeExceeded("application_javascript_too_large", "combined application chunks exceed the application budget", {
      size_bytes: applicationJavaScriptBytes,
      limit_bytes: LIMITS.applicationJavaScript,
    });
  }

  assertReference(entry, reactVendor, "entry_missing_react_vendor_reference", "entry");
  assertReference(entry, graphVendor, "entry_missing_graph_vendor_reference", "entry");
  assertReference(graphWorker, elkWorker, "graph_worker_missing_elk_reference", "graph-layout.worker");

  return {
    schema_version: SCHEMA_VERSION,
    status: warnings.length ? "WARN" : "PASS",
    policy,
    policy_decision: policy === "private-v0.2-size-observation" ? "2026-09-08 user decision: prioritize working functionality; private v0.2 size limits are observation-only, correctness checks remain blocking" : "historical frozen hard size limits",
    warnings,
    dist: dist,
    entry: { path: entry.path, size_bytes: entry.size_bytes },
    vendors: {
      react: { path: reactVendor.path, size_bytes: reactVendor.size_bytes },
      graph: { path: graphVendor.path, size_bytes: graphVendor.size_bytes },
    },
    graph_worker: { path: graphWorker.path, size_bytes: graphWorker.size_bytes },
    elk_worker: { path: elkWorker.path, size_bytes: elkWorker.size_bytes },
    graph_path_bytes: graphPathBytes,
    javascript_asset_count: javascript.length,
    application_javascript_bytes: applicationJavaScriptBytes,
    limits: LIMITS,
  };
}

function parseArgs(args) {
  let dist;
  let receipt;
  let policy = "strict-v0.1";
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--dist") {
      dist = args[++index];
      if (!dist) fail("invalid_arguments", "--dist requires a path");
    } else if (argument === "--receipt") {
      receipt = args[++index];
      if (!receipt) fail("invalid_arguments", "--receipt requires a path");
    } else if (argument === "--policy") {
      policy = args[++index];
      if (!["strict-v0.1", "private-v0.2-size-observation"].includes(policy)) fail("invalid_policy", "unknown bundle policy");
    } else if (argument === "--help" || argument === "-h") {
      return { help: true };
    } else {
      fail("invalid_arguments", `unknown argument: ${argument}`);
    }
  }
  if (!dist) fail("invalid_arguments", "--dist is required");
  return { dist: resolve(dist), receipt: receipt ? resolve(receipt) : undefined, policy };
}

function writeReceipt(path, result) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, `${JSON.stringify(result)}\n`, "utf8");
}

function main() {
  let options;
  try {
    options = parseArgs(process.argv.slice(2));
    if (options.help) {
      process.stdout.write("usage: node verify-frontend-bundles.mjs --dist DIST [--receipt RECEIPT] [--policy strict-v0.1|private-v0.2-size-observation]\n");
      return;
    }
    const result = verifyFrontendBundles(options.dist, options.policy);
    if (options.receipt) writeReceipt(options.receipt, result);
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } catch (error) {
    const result = {
      schema_version: SCHEMA_VERSION,
      status: "FAIL",
      policy: options?.policy ?? "strict-v0.1",
      code: error?.code ?? "verification_failed",
      message: error instanceof Error ? error.message : "verification failed",
      ...(error?.details ?? {}),
    };
    if (options?.receipt) writeReceipt(options.receipt, result);
    process.stdout.write(`${JSON.stringify(result)}\n`);
    process.exitCode = 1;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
