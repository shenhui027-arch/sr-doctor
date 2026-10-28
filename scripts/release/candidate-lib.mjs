import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, readFileSync, realpathSync, renameSync, statSync, lstatSync, copyFileSync, writeFileSync } from "node:fs";
import { basename, dirname, extname, isAbsolute, join, normalize, relative, resolve, sep } from "node:path";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

export const CANDIDATE_SCHEMA = "sr-doctor.candidate/v0.1";
// Outside the repository: SR_DOCTOR_WORK_ROOT, or sr-doctor under the system temp directory.
export const DEFAULT_RELEASE_ROOT = join(process.env.SR_DOCTOR_WORK_ROOT ?? join(tmpdir(), "sr-doctor"), "release");
const MAX_LICENSE_BYTES = 2 * 1024 * 1024;

function fail(message) {
  throw new Error(message);
}

function posixPath(value) {
  return value.split(sep).join("/");
}

function stable(value) {
  if (Array.isArray(value)) return value.map(stable);
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.keys(value).sort().map((key) => [key, stable(value[key])]));
  }
  return value;
}

export function stableJson(value) {
  return `${JSON.stringify(stable(value), null, 2)}\n`;
}

export function sha256Bytes(value) {
  return createHash("sha256").update(value).digest("hex");
}

export function sha256File(file) {
  return sha256Bytes(readFileSync(file));
}

function ensureDirectory(path) {
  mkdirSync(path, { recursive: true });
}

function ensureExistingFile(path, label) {
  if (!existsSync(path) || !statSync(path).isFile()) fail(`${label} must be an existing file: ${path}`);
}

function ensureSafeRelativePath(path, label) {
  const normal = posixPath(normalize(path));
  if (!normal || normal === "." || isAbsolute(normal) || normal.startsWith("../") || normal.includes("/../")) {
    fail(`${label} is not a safe relative path: ${path}`);
  }
  return normal;
}

function ensureWithin(root, child, label) {
  const rootReal = realpathSync(root);
  const childReal = realpathSync(child);
  const childRelative = relative(rootReal, childReal);
  if (childRelative === "" || (!childRelative.startsWith(`..${sep}`) && childRelative !== ".." && !isAbsolute(childRelative))) return;
  fail(`${label} escapes its declared root: ${child}`);
}

export function listRegularFiles(root) {
  ensureExistingDirectory(root, "directory");
  const result = [];
  const walk = (current) => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const target = join(current, entry.name);
      if (entry.isSymbolicLink()) fail(`candidate input contains a symbolic link: ${target}`);
      if (entry.isDirectory()) walk(target);
      else if (entry.isFile()) result.push(target);
      else fail(`candidate input contains a non-regular entry: ${target}`);
    }
  };
  walk(root);
  return result.sort((left, right) => posixPath(relative(root, left)).localeCompare(posixPath(relative(root, right))));
}

function ensureExistingDirectory(path, label) {
  if (!existsSync(path) || !statSync(path).isDirectory()) fail(`${label} must be an existing directory: ${path}`);
}

function frontendPathAllowed(path) {
  return path === "index.html"
    || /^assets\/[^/]+\.(?:js|css|woff2|svg)$/i.test(path)
    || /^licenses\/(?:Inter-OFL-1\.1|JetBrains-Mono-OFL-1\.1|Noto-Sans-SC-OFL-1\.1)\.txt$/i.test(path);
}

export function candidatePathAllowed(path) {
  if (["LICENSE", "NOTICE", "doctor-api.exe", "start.ps1", "candidate-manifest.json"].includes(path)) return true;
  if (path.startsWith("frontend/")) return frontendPathAllowed(path.slice("frontend/".length));
  if ([
    "docs/LICENSE-STATUS.md",
    "docs/ROLLBACK.md",
    "docs/SECURITY.md",
    "docs/SUPPORT.md",
    "docs/THIRD-PARTY-NOTICES.md",
  ].includes(path)) return true;
  if (/^sbom\/(?:cargo-dependencies|npm-dependencies|license-report|cyclonedx-1\.5)\.json$/.test(path)) return true;
  if (/^sbom\/(?:Cargo\.lock|frontend-package-lock\.json)$/.test(path)) return true;
  return /^licenses\/(?:rust|npm)\/[a-z0-9._-]+\.txt$/i.test(path);
}

function assertCandidatePath(path) {
  const safe = ensureSafeRelativePath(path, "candidate path");
  if (!candidatePathAllowed(safe)) fail(`candidate allowlist rejected: ${safe}`);
  if (/northstar/i.test(safe)) fail(`candidate must not include an old reference resource: ${safe}`);
  return safe;
}

function copyInto(source, destinationRoot, destinationRelative) {
  const safe = assertCandidatePath(destinationRelative);
  ensureExistingFile(source, "candidate source");
  const target = join(destinationRoot, ...safe.split("/"));
  ensureDirectory(dirname(target));
  copyFileSync(source, target, 0);
  return target;
}

function slug(value) {
  const base = value.replace(/[^a-z0-9._-]+/gi, "_").replace(/^_+|_+$/g, "") || "package";
  return `${base.slice(0, 48)}-${sha256Bytes(value).slice(0, 12)}`;
}

function command(command, args, cwd, encoding = "utf8", environment = null) {
  const npmOnWindows = process.platform === "win32" && command === "npm";
  const executable = npmOnWindows ? (process.env.ComSpec ?? "cmd.exe") : command;
  const executableArgs = npmOnWindows
    ? ["/d", "/s", "/c", "npm.cmd", ...args]
    : args;
  const result = spawnSync(executable, executableArgs, {
    cwd,
    encoding,
    windowsHide: true,
    env: environment ? { ...process.env, ...environment } : process.env,
  });
  if (result.error) fail(`command failed to start: ${command} ${args.join(" ")}: ${result.error.message}`);
  if (result.status !== 0) {
    const stderr = Buffer.isBuffer(result.stderr) ? result.stderr.toString("utf8") : result.stderr;
    fail(`command failed (${result.status}): ${command} ${args.join(" ")}\n${stderr || ""}`.trim());
  }
  return result.stdout;
}

function commandText(commandName, args, cwd) {
  return String(command(commandName, args, cwd)).trim();
}

function packageVersion(record, label) {
  if (!record || typeof record.version !== "string" || !record.version.trim()) {
    fail(`${label} version is missing`);
  }
  return record.version;
}

function lockProjectVersion(lock, label) {
  const version = packageVersion(lock, `${label} lockfile`);
  const rootVersion = packageVersion(lock?.packages?.[""], `${label} lockfile root package`);
  if (version !== rootVersion) fail(`${label} lockfile top-level and root package versions disagree`);
  return version;
}

// `doctor-api` is the product binary. Its Cargo metadata version is the sole
// product-version source; npm manifests and locks are checked against it rather
// than defining another release-version constant.
export function productVersionFromSourceMetadata({ cargoMetadata, rootPackage, rootPackageLock, frontendPackage, frontendPackageLock }) {
  if (!cargoMetadata || !Array.isArray(cargoMetadata.packages) || !Array.isArray(cargoMetadata.workspace_members)) {
    fail("Cargo metadata does not describe a workspace package set");
  }
  const workspaceMembers = new Set(cargoMetadata.workspace_members);
  const workspacePackages = cargoMetadata.packages.filter((pkg) => workspaceMembers.has(pkg.id));
  const doctorApi = workspacePackages.filter((pkg) => pkg.name === "doctor-api");
  if (doctorApi.length !== 1) fail("Cargo metadata must contain exactly one workspace doctor-api package");
  const productVersion = packageVersion(doctorApi[0], "doctor-api");
  const driftedWorkspacePackage = workspacePackages.find((pkg) => packageVersion(pkg, `workspace package ${pkg.name}`) !== productVersion);
  if (driftedWorkspacePackage) {
    fail(`workspace package version disagrees with doctor-api: ${driftedWorkspacePackage.name}`);
  }
  const evidence = [
    ["root package.json", packageVersion(rootPackage, "root package.json")],
    ["root package-lock.json", lockProjectVersion(rootPackageLock, "root package")],
    ["frontend package.json", packageVersion(frontendPackage, "frontend package.json")],
    ["frontend package-lock.json", lockProjectVersion(frontendPackageLock, "frontend package")],
  ];
  const mismatch = evidence.find(([, version]) => version !== productVersion);
  if (mismatch) fail(`${mismatch[0]} version ${mismatch[1]} disagrees with doctor-api ${productVersion}`);
  return productVersion;
}

function productVersionFromSourceRoot(sourceRoot, cargoMetadata = null) {
  const metadata = cargoMetadata ?? JSON.parse(commandText("cargo", ["metadata", "--locked", "--format-version", "1"], sourceRoot));
  return productVersionFromSourceMetadata({
    cargoMetadata: metadata,
    rootPackage: JSON.parse(readFileSync(join(sourceRoot, "package.json"), "utf8")),
    rootPackageLock: JSON.parse(readFileSync(join(sourceRoot, "package-lock.json"), "utf8")),
    frontendPackage: JSON.parse(readFileSync(join(sourceRoot, "frontend", "package.json"), "utf8")),
    frontendPackageLock: JSON.parse(readFileSync(join(sourceRoot, "frontend", "package-lock.json"), "utf8")),
  });
}

function productReleaseLine(productVersion) {
  const match = /^(\d+)\.(\d+)\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.exec(productVersion);
  if (!match) fail(`doctor-api product version is not a supported semver value: ${productVersion}`);
  return `${match[1]}.${match[2]}`;
}

function gitProvenance(sourceRoot) {
  const head = commandText("git", ["rev-parse", "HEAD"], sourceRoot);
  const commitFields = commandText("git", ["show", "-s", "--format=%an%x00%ae%x00%s", "HEAD"], sourceRoot).split("\0");
  if (commitFields.length !== 3 || commitFields.some((field) => !field)) fail("Git commit author/message provenance is incomplete");
  const symbolic = spawnSync("git", ["symbolic-ref", "--quiet", "--short", "HEAD"], {
    cwd: sourceRoot,
    encoding: "utf8",
    windowsHide: true,
  });
  if (symbolic.error || ![0, 1].includes(symbolic.status)) fail("Git ref provenance could not be resolved");
  const rawStatus = command("git", ["status", "--porcelain=v1", "--untracked-files=all", "--no-renames", "-z"], sourceRoot, null);
  const entries = [];
  for (const record of rawStatus.toString("utf8").split("\0")) {
    if (!record) continue;
    const status = record.slice(0, 2);
    const sourceRelative = ensureSafeRelativePath(record.slice(3), "git status path");
    const absolute = resolve(sourceRoot, ...sourceRelative.split("/"));
    let contentSha256 = null;
    if (existsSync(absolute)) {
      const info = lstatSync(absolute);
      if (info.isSymbolicLink()) contentSha256 = sha256Bytes(`symlink:${readFileSync(absolute, "utf8")}`);
      else if (info.isFile()) contentSha256 = sha256File(absolute);
      else fail(`git status contains a non-file candidate: ${sourceRelative}`);
    }
    entries.push({ status, path: sourceRelative, content_sha256: contentSha256 });
  }
  entries.sort((left, right) => `${left.path}\0${left.status}`.localeCompare(`${right.path}\0${right.status}`));
  return {
    git_head: head,
    git_detached: symbolic.status === 1,
    git_ref: symbolic.status === 0 ? symbolic.stdout.trim() : null,
    commit_author_name: commitFields[0],
    commit_author_email: commitFields[1],
    commit_message: commitFields[2],
    dirty: entries.length > 0,
    dirty_entry_count: entries.length,
    dirty_status_sha256: sha256Bytes(rawStatus),
    dirty_source_snapshot_sha256: sha256Bytes(stableJson({ git_head: head, entries })),
  };
}

function fileDescriptor(root, relativePath) {
  const safe = ensureSafeRelativePath(relativePath, "manifest file path");
  const absolute = resolve(root, ...safe.split("/"));
  ensureWithin(root, absolute, "manifest file");
  ensureExistingFile(absolute, "manifest file");
  return { path: safe, size_bytes: statSync(absolute).size, sha256: sha256File(absolute) };
}

function lockDescriptors(sourceRoot) {
  const declarations = [
    ["Cargo.lock", "Cargo.lock"],
    ["frontend-package-lock.json", "frontend/package-lock.json"],
  ];
  return declarations.map(([id, relativePath]) => {
    const source = resolve(sourceRoot, ...relativePath.split("/"));
    ensureExistingFile(source, `lockfile ${id}`);
    return { id, source_path: relativePath, sha256: sha256File(source), size_bytes: statSync(source).size };
  });
}

function licenseSourceFiles(packageDirectory, explicitLicenseFile = null) {
  if (!packageDirectory || !existsSync(packageDirectory) || !statSync(packageDirectory).isDirectory()) return [];
  const candidates = new Set();
  if (explicitLicenseFile && existsSync(explicitLicenseFile) && statSync(explicitLicenseFile).isFile()) candidates.add(resolve(explicitLicenseFile));
  for (const entry of readdirSync(packageDirectory, { withFileTypes: true })) {
    if (!entry.isFile()) continue;
    if (/^(?:license|copying|notice|copyright)(?:[._-].*)?$/i.test(entry.name)) candidates.add(resolve(packageDirectory, entry.name));
  }
  return [...candidates].filter((path) => statSync(path).size <= MAX_LICENSE_BYTES).sort();
}

function copyLicenseTexts({ sourceFiles, destinationRoot, family, packageIdentity }) {
  const copied = [];
  for (let index = 0; index < sourceFiles.length; index += 1) {
    const source = sourceFiles[index];
    const text = readFileSync(source);
    const targetRelative = `licenses/${family}/${slug(`${packageIdentity}:${index}:${basename(source)}`)}.txt`;
    copyInto(source, destinationRoot, targetRelative);
    copied.push({ package_path: packageIdentity, file: basename(source), candidate_path: targetRelative, sha256: sha256Bytes(text), size_bytes: text.length });
  }
  return copied;
}

function auditedCargoLicenseEvidence(sourceRoot) {
  const manifestPath = join(sourceRoot, "third_party", "license-evidence.json");
  ensureExistingFile(manifestPath, "audited third-party license evidence");
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  if (manifest.schema_version !== 1 || !Array.isArray(manifest.packages)) fail("unsupported third-party license evidence schema");
  const records = new Map();
  for (const entry of manifest.packages) {
    if (entry.ecosystem !== "cargo" || !entry.name || !entry.version || !entry.source || !entry.license_expression || !Array.isArray(entry.evidence)) {
      fail("invalid audited Cargo license evidence entry");
    }
    const key = `${entry.name}@${entry.version} (${entry.source})`;
    if (records.has(key)) fail(`duplicate audited Cargo license evidence: ${key}`);
    const files = entry.evidence.map((evidence) => {
      const relativePath = ensureSafeRelativePath(evidence.path, "license evidence path");
      const source = resolve(sourceRoot, ...relativePath.split("/"));
      ensureExistingFile(source, `license evidence ${relativePath}`);
      ensureWithin(sourceRoot, source, "license evidence");
      if (!/^[a-f0-9]{64}$/.test(evidence.sha256) || sha256File(source) !== evidence.sha256) {
        fail(`license evidence hash mismatch: ${relativePath}`);
      }
      return source;
    });
    records.set(key, { licenseExpression: entry.license_expression, files });
  }
  return records;
}

function sanitizeCargoMetadata(metadata, destinationRoot, sourceRoot) {
  const packageIdentity = (pkg) => `${pkg.name}@${pkg.version}${pkg.source ? ` (${pkg.source})` : " (workspace)"}`;
  const identities = new Map(metadata.packages.map((pkg) => [pkg.id, packageIdentity(pkg)]));
  const auditedEvidence = auditedCargoLicenseEvidence(sourceRoot);
  const licenseTexts = [];
  const packages = metadata.packages.map((pkg) => {
    const manifestDirectory = pkg.manifest_path ? dirname(pkg.manifest_path) : null;
    let sources = licenseSourceFiles(manifestDirectory, pkg.license_file ?? null);
    let licenseTextStatus = sources.length ? "copied_from_local_cache" : "not_found";
    if (!sources.length && !pkg.source) {
      sources = [join(sourceRoot, "LICENSE")];
      ensureExistingFile(sources[0], "workspace root LICENSE");
      licenseTextStatus = "copied_from_workspace_license";
    } else if (!sources.length) {
      const audited = auditedEvidence.get(packageIdentity(pkg));
      if (audited) {
        if (audited.licenseExpression !== pkg.license) fail(`audited license expression mismatch: ${packageIdentity(pkg)}`);
        sources = audited.files;
        licenseTextStatus = "copied_from_audited_evidence";
      }
    }
    const copied = copyLicenseTexts({ sourceFiles: sources, destinationRoot, family: "rust", packageIdentity: packageIdentity(pkg) });
    licenseTexts.push(...copied);
    return {
      identity: packageIdentity(pkg),
      name: pkg.name,
      version: pkg.version,
      source: pkg.source ?? "workspace",
      license_expression: pkg.license ?? null,
      license_file_declared: pkg.license_file ? basename(pkg.license_file) : null,
      license_text_status: copied.length ? licenseTextStatus : "not_found",
      license_texts: copied.map((entry) => entry.candidate_path),
      dependencies: pkg.dependencies.map((dependency) => ({
        name: dependency.name,
        source: dependency.source ?? "workspace",
        requirement: dependency.req,
        kind: dependency.kind ?? "normal",
        optional: dependency.optional,
        features: [...dependency.features].sort(),
      })).sort((left, right) => `${left.name}\0${left.source}`.localeCompare(`${right.name}\0${right.source}`)),
    };
  }).sort((left, right) => left.identity.localeCompare(right.identity));
  const resolveNodes = (metadata.resolve?.nodes ?? []).map((node) => ({
    package: identities.get(node.id) ?? node.id,
    dependencies: (node.dependencies ?? []).map((id) => identities.get(id) ?? id).sort(),
  })).sort((left, right) => left.package.localeCompare(right.package));
  return {
    generated_from: "cargo metadata --locked --format-version 1",
    packages,
    resolve: resolveNodes,
    license_texts: licenseTexts.sort((left, right) => left.candidate_path.localeCompare(right.candidate_path)),
  };
}

function normalizeNpmLicense(value) {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) return value.map((entry) => typeof entry === "string" ? entry : entry?.type ?? null).filter(Boolean).join(" OR ") || null;
  if (value && typeof value === "object" && typeof value.type === "string") return value.type;
  return null;
}

function sanitizeNpmLock(frontendRoot, destinationRoot) {
  const lockPath = join(frontendRoot, "package-lock.json");
  const lock = JSON.parse(readFileSync(lockPath, "utf8"));
  const packageRecords = Object.entries(lock.packages ?? {}).filter(([packagePath, entry]) => packagePath !== "" && entry.dev !== true);
  const licenseTexts = [];
  const packages = packageRecords.map(([packagePath, entry]) => {
    const safeLockPath = ensureSafeRelativePath(packagePath, "npm package path");
    if (!safeLockPath.startsWith("node_modules/")) fail(`npm package path is outside node_modules: ${packagePath}`);
    const packageDirectory = resolve(frontendRoot, ...safeLockPath.split("/"));
    const packageJsonPath = join(packageDirectory, "package.json");
    let installed = null;
    if (existsSync(packageJsonPath) && statSync(packageJsonPath).isFile()) installed = JSON.parse(readFileSync(packageJsonPath, "utf8"));
    const packageName = installed?.name ?? entry.name ?? safeLockPath.split("node_modules/").at(-1);
    const identity = `${packageName}@${entry.version ?? installed?.version ?? "unknown"}`;
    const copied = copyLicenseTexts({
      sourceFiles: licenseSourceFiles(packageDirectory),
      destinationRoot,
      family: "npm",
      packageIdentity: identity,
    });
    licenseTexts.push(...copied);
    return {
      package_path: safeLockPath,
      name: packageName,
      version: entry.version ?? installed?.version ?? null,
      resolved: entry.resolved ?? null,
      integrity: entry.integrity ?? null,
      license_expression: normalizeNpmLicense(entry.license ?? installed?.license ?? installed?.licenses),
      installed_package_json: Boolean(installed),
      license_text_status: copied.length ? "copied_from_local_node_modules" : "not_found_locally",
      license_texts: copied.map((item) => item.candidate_path),
    };
  }).sort((left, right) => left.package_path.localeCompare(right.package_path));
  return {
    generated_from: "frontend/package-lock.json production/runtime entries plus installed frontend/node_modules license texts",
    lockfile_version: lock.lockfileVersion ?? null,
    project: {
      name: lock.name ?? null,
      version: lock.version ?? null,
      dependencies: stable(lock.packages?.[""]?.dependencies ?? {}),
    },
    packages,
    license_texts: licenseTexts.sort((left, right) => left.candidate_path.localeCompare(right.candidate_path)),
  };
}

function writeJson(target, value) {
  ensureDirectory(dirname(target));
  writeFileSync(target, stableJson(value), "utf8");
}

export function generatedSupportDocument(productVersion) {
  const native = productReleaseLine(productVersion) === "0.2";
  return `# v${productVersion} Candidate support and runbook\n\n## Supported scope\n\n- Local, single-user application on \`127.0.0.1:8000\`; no HA, multi-user, RBAC, multi-tenancy or public bind.\n- Offline Profile import plus optional StarRocks exact \`3.5.14\` connections. Unregistered versions are reported as unsupported and never borrow the nearest VersionPack.\n- The supported analysis loop is Profile acquisition, deterministic findings/recommendations, controlled user-side rerun, and Compare. Production-safe remains INCONCLUSIVE.\n\n## Controlled database boundary\n\n- Fixed server operations: Audit query, \`SHOW PROFILELIST\`, \`get_query_profile\`, \`SHOW PLAN ADVISOR\`, and ordinary \`EXPLAIN\`.\n- Runtime Top additionally accepts one bounded \`SELECT\` or \`WITH ... SELECT\`, applies a fixed 10-second database timeout, discards result rows, retrieves the exact Query Profile, and analyzes its plan/runtime projections.\n${native ? "- Native execution is a separate explicit confirmation: one bounded SELECT/WITH is wrapped in exactly one EXPLAIN ANALYZE with a fixed 10-second timeout. It returns native official evidence; it does not first execute a bare SELECT. Standard Runtime Top executes its SELECT once and never issues EXPLAIN ANALYZE. Neither method automatically retries or falls back to the other.\n- Multi-statements, DDL, DML, INSERT, Advisor mutation and automatic SQL/parameter changes are rejected." : "- Multi-statements, EXPLAIN ANALYZE, DDL, DML, INSERT, Advisor mutation and automatic SQL/parameter changes are rejected."}\n- Remote connections require TLS verify-full. Plaintext is limited to one-time loopback Docker fixtures.\n\n## Credentials, privacy and limits\n\n- Credentials live only in the process session, are never returned to the browser, persisted, logged or placed in the Candidate, and are removed on disconnect/expiry/process exit.\n- SQL/Profile/Audit source text is not telemetry. Logs and exported evidence use typed, allowlisted projections.\n- Input byte/line/node/depth/metric budgets fail closed. Missing, invalid, partial, overflow and zero remain distinct.\n\n## Start and verify\n\n1. Verify \`release-manifest.json\`, the ZIP SHA-256 and \`candidate-manifest.json\` before use.\n2. Keep the immutable run directory intact; do not mix files from different Candidates.\n3. Run \`candidate\\start.ps1\`; open \`http://127.0.0.1:8000\`.\nUse start.ps1 -Port 18054 for another local port; direct binary launches accept DOCTOR_PORT. The manifest bind_address is the default.\n4. Confirm the capability endpoint reports the expected Candidate label and Git commit.\n`;
}

function generatedRollbackDocument() {
  return `# Candidate rollback and deletion\n\n1. Disconnect active database sessions in the UI, then stop the \`doctor-api.exe\` process launched by \`start.ps1\`.\n2. Verify port \`127.0.0.1:8000\` is no longer listening (use your configured port if changed) and no Candidate-owned process remains.\n3. Preserve the release/candidate manifests and hashes required for audit; then remove the whole immutable run directory. Never delete or merge individual payload files.\n4. Verify the prior Candidate's release manifest, Candidate manifest and ZIP hashes. Start only its own \`start.ps1\`.\n5. Confirm the capability endpoint reports the prior label/commit and repeat the offline import smoke check.\n6. Database credentials, Profile text and Audit rows are not stored in the package; rollback does not require database mutation.\n`;
}

export function generatedSecurityDocument(productVersion) {
  const native = productReleaseLine(productVersion) === "0.2";
  return `# Security and threat boundary\n\n- Trust boundary: local browser and loopback API are one single-user process boundary; imported Profile, Audit fields and database responses are untrusted input.\n- The frontend renders backend DTOs and does not parse raw Profile, recompute rules/topology, or use untrusted HTML.\n- Outbound database hosts are explicit MySQL host/port values, never URLs or file paths. Remote TLS verification cannot be downgraded.\n- Database access is read-only except the explicit Runtime Top path: it accepts exactly one bounded \`SELECT\` or \`WITH ... SELECT\`, uses a fixed 10-second timeout, discards rows, and fetches only that execution's Profile. There is no arbitrary SQL endpoint.\n${native ? "- The separate, explicitly confirmed native path issues one EXPLAIN ANALYZE wrapper, with the same fixed 10-second timeout and no preliminary bare SELECT. Standard/native modes never automatically retry or fall back to one another; unknown execution outcome is not permission to submit again.\n" : "- Native EXPLAIN ANALYZE execution is not offered in this release.\n"}- Candidate packaging rejects source fixtures, production corpus, credentials and reference assets.\n- Production load impact, production TLS deployment, HA and organizational access controls are outside v${productVersion} validation; Production-safe is INCONCLUSIVE.\n`;
}

function generatedLicenseStatusDocument(licenseReport) {
  return `# License status\n\n- The repository and Candidate include the project Apache-2.0 \`LICENSE\` and owner-approved \`NOTICE\`. Redistribution status: READY.\n- Dependency evidence covers the locked Cargo workspace closure and npm production/runtime entries, using installed package license texts and exact-commit audited evidence. Missing expressions: ${licenseReport.missing_license_expression.length}; missing license texts: ${licenseReport.missing_local_license_text.length}.\n- Evidence is not inferred. See \`sbom/license-report.json\`, \`sbom/cyclonedx-1.5.json\`, \`licenses/\` and \`docs/THIRD-PARTY-NOTICES.md\`.\n`;
}

function generatedThirdPartyNoticesDocument() {
  return `# Third-party notices evidence\n\nThis Candidate includes a CycloneDX 1.5 SBOM, the locked Cargo workspace closure, npm production/runtime entries, and package license texts copied from installed dependency packages or exact-commit audited evidence. Package-level identities, expressions, hashes, and evidence paths are under \`sbom/\` and \`licenses/\`.\n`;
}

function writeGeneratedDocs(destinationRoot, licenseReport, productVersion) {
  ensureDirectory(join(destinationRoot, "docs"));
  writeFileSync(join(destinationRoot, "docs", "SUPPORT.md"), generatedSupportDocument(productVersion), "utf8");
  writeFileSync(join(destinationRoot, "docs", "ROLLBACK.md"), generatedRollbackDocument(), "utf8");
  writeFileSync(join(destinationRoot, "docs", "SECURITY.md"), generatedSecurityDocument(productVersion), "utf8");
  writeFileSync(join(destinationRoot, "docs", "LICENSE-STATUS.md"), generatedLicenseStatusDocument(licenseReport), "utf8");
  writeFileSync(join(destinationRoot, "docs", "THIRD-PARTY-NOTICES.md"), generatedThirdPartyNoticesDocument(), "utf8");
}

function writeStartScript(destinationRoot) {
  const content = `# Local-only Candidate launcher. Default: http://127.0.0.1:8000
param([ValidateRange(1, 65535)][int]$Port = 8000)
$ErrorActionPreference = 'Stop'
$packageRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$binary = Join-Path $packageRoot 'doctor-api.exe'
$frontend = Join-Path $packageRoot 'frontend'
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw "Missing Candidate binary: $binary" }
if (-not (Test-Path -LiteralPath $frontend -PathType Container)) { throw "Missing Candidate frontend directory: $frontend" }
$previousPort = $env:DOCTOR_PORT
$previousFrontend = $env:DOCTOR_FRONTEND_DIST
try {
  $env:DOCTOR_PORT = [string]$Port
  $env:DOCTOR_FRONTEND_DIST = $frontend
  Write-Host "SR Doctor Candidate: http://127.0.0.1:$Port"
  & $binary
  $code = $LASTEXITCODE
} finally {
  $env:DOCTOR_PORT = $previousPort
  $env:DOCTOR_FRONTEND_DIST = $previousFrontend
}
exit $code
`;
  writeFileSync(join(destinationRoot, "start.ps1"), content, "utf8");
}
function copyFrontend(frontendDist, candidateRoot) {
  ensureExistingDirectory(frontendDist, "frontend dist");
  for (const source of listRegularFiles(frontendDist)) {
    const rel = posixPath(relative(frontendDist, source));
    if (!frontendPathAllowed(rel)) fail(`frontend dist allowlist rejected: ${rel}`);
    if (/northstar/i.test(rel)) fail(`frontend dist contains an old reference asset: ${rel}`);
    copyInto(source, candidateRoot, `frontend/${rel}`);
  }
  ensureExistingFile(join(candidateRoot, "frontend", "index.html"), "frontend index.html");
}

function createZip(candidateRoot, archivePath) {
  const powershell = [
    "Add-Type -AssemblyName System.IO.Compression.FileSystem",
    `[System.IO.Compression.ZipFile]::CreateFromDirectory('${candidateRoot.replace(/'/g, "''")}', '${archivePath.replace(/'/g, "''")}', [System.IO.Compression.CompressionLevel]::Optimal, $true)`,
  ].join("; ");
  command("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", powershell], dirname(candidateRoot));
}

export function zipEntries(archivePath) {
  ensureExistingFile(archivePath, "archive");
  const powershell = [
    "Add-Type -AssemblyName System.IO.Compression.FileSystem",
    `$zip = [System.IO.Compression.ZipFile]::OpenRead('${archivePath.replace(/'/g, "''")}')`,
    "try { $zip.Entries | ForEach-Object { $_.FullName } } finally { $zip.Dispose() }",
  ].join("; ");
  return commandText("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", powershell], dirname(archivePath))
    .split(/\r?\n/).filter(Boolean).map(posixPath).sort();
}

export function zipEntryDescriptors(archivePath, limits) {
  ensureExistingFile(archivePath, "archive");
  const maxEntries = Number(limits?.maxEntries);
  const maxUncompressedBytes = Number(limits?.maxUncompressedBytes);
  if (!Number.isSafeInteger(maxEntries) || maxEntries < 1
    || !Number.isSafeInteger(maxUncompressedBytes) || maxUncompressedBytes < 0) {
    fail("archive verification limits must be safe non-negative integers");
  }
  const escapedArchive = archivePath.replace(/'/g, "''");
  const powershell = [
    "Add-Type -AssemblyName System.IO.Compression.FileSystem",
    `$zip = [System.IO.Compression.ZipFile]::OpenRead('${escapedArchive}')`,
    `$maxEntries = ${maxEntries}`,
    `$maxBytes = [decimal]${maxUncompressedBytes}`,
    "$exact = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)",
    "$folded = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)",
    "$rows = @()",
    "try {",
    "  if ($zip.Entries.Count -gt $maxEntries) { throw 'archive entry count exceeds package manifest' }",
    "  $total = [decimal]0",
    "  foreach ($entry in $zip.Entries) {",
    "    $name = $entry.FullName.Replace('\\', '/')",
    "    $parts = $name.Split('/')",
    "    if (-not $name.StartsWith('candidate/') -or $name.EndsWith('/') -or $name.StartsWith('/') -or $name -match '^[A-Za-z]:' -or $parts -contains '..' -or $parts -contains '' -or -not $exact.Add($name) -or -not $folded.Add($name)) { throw ('unsafe or duplicate archive entry: ' + $entry.FullName) }",
    "    $total += [decimal]$entry.Length",
    "    if ($total -gt $maxBytes) { throw 'archive uncompressed size exceeds package manifest' }",
    "  }",
    "  foreach ($entry in $zip.Entries) {",
    "    $stream = $entry.Open()",
    "    try {",
    "      $sha = [System.Security.Cryptography.SHA256]::Create()",
    "      try { $hash = $sha.ComputeHash($stream) } finally { $sha.Dispose() }",
    "      $rows += [pscustomobject]@{ path = $entry.FullName.Replace('\\', '/'); size_bytes = $entry.Length; compressed_size_bytes = $entry.CompressedLength; sha256 = (($hash | ForEach-Object { $_.ToString('x2') }) -join '') }",
    "    } finally { $stream.Dispose() }",
    "  }",
    "  @($rows) | ConvertTo-Json -Compress -Depth 3",
    "} finally { $zip.Dispose() }",
  ].join("; ");
  const output = commandText("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", powershell], dirname(archivePath));
  const parsed = JSON.parse(output || "[]");
  return Array.isArray(parsed) ? parsed : [parsed];
}

function expectedZipEntries(candidateRoot) {
  return listRegularFiles(candidateRoot).map((file) => `candidate/${posixPath(relative(candidateRoot, file))}`).sort();
}

function packageFiles(candidateRoot) {
  return listRegularFiles(candidateRoot)
    .map((source) => posixPath(relative(candidateRoot, source)))
    .filter((path) => path !== "candidate-manifest.json")
    .map((path) => fileDescriptor(candidateRoot, path));
}

function assertSafeCandidateContents(candidateRoot) {
  const all = listRegularFiles(candidateRoot).map((file) => posixPath(relative(candidateRoot, file)));
  for (const path of all) assertCandidatePath(path);
  const oldResource = all.find((path) => /northstar/i.test(path));
  if (oldResource) fail(`candidate contains an old reference resource: ${oldResource}`);
  for (const path of all.filter((value) => /^(?:frontend\/.*\.(?:html|js|css|svg)|docs\/.*\.(?:md|txt)|sbom\/.*\.json|start\.ps1)$/i.test(value))) {
    const contents = readFileSync(join(candidateRoot, ...path.split("/")), "utf8");
    if (/northstar/i.test(contents)) fail(`candidate frontend contains old reference text: ${path}`);
    if (/-----BEGIN(?: [A-Z]+)? PRIVATE KEY-----/i.test(contents) || /\bAKIA[0-9A-Z]{16}\b/.test(contents)) {
      fail(`candidate contains a secret-shaped value: ${path}`);
    }
    // Bundled frameworks legitimately contain property maps such as
    // `password:!0,range:!0`.  Treat only token-shaped assigned values as a
    // credential, not arbitrary minified syntax following a sensitive key.
    if (/\b(?:password|passwd|api[_-]?key|access[_-]?token|secret)\s*[:=]\s*(?:["'](?!redacted\b|placeholder\b|example\b|<)[A-Za-z0-9_+./=-]{12,}["']|(?!(?:true|false|null|undefined)\b)[A-Za-z0-9_+./=-]{16,})/i.test(contents)) {
      fail(`candidate contains a credential assignment: ${path}`);
    }
  }
  const startup = readFileSync(join(candidateRoot, "start.ps1"), "utf8");
  if (!startup.includes("127.0.0.1:8000") || /0\.0\.0\.0|::0|--bind/i.test(startup)) {
    fail("Candidate start.ps1 does not preserve loopback-only launch semantics");
  }
}

function writeLicenseArtifacts(candidateRoot, sourceRoot) {
  const cargoRaw = commandText("cargo", ["metadata", "--locked", "--format-version", "1"], sourceRoot);
  const cargoMetadata = JSON.parse(cargoRaw);
  const productVersion = productVersionFromSourceRoot(sourceRoot, cargoMetadata);
  const cargo = sanitizeCargoMetadata(cargoMetadata, candidateRoot, sourceRoot);
  const npm = sanitizeNpmLock(join(sourceRoot, "frontend"), candidateRoot);
  writeJson(join(candidateRoot, "sbom", "cargo-dependencies.json"), cargo);
  writeJson(join(candidateRoot, "sbom", "npm-dependencies.json"), npm);
  copyInto(join(sourceRoot, "Cargo.lock"), candidateRoot, "sbom/Cargo.lock");
  copyInto(join(sourceRoot, "frontend", "package-lock.json"), candidateRoot, "sbom/frontend-package-lock.json");
  const packageCount = cargo.packages.length + npm.packages.length;
  const licenseReport = {
    generated_from: [cargo.generated_from, npm.generated_from],
    project_license: fileDescriptor(candidateRoot, "LICENSE"),
    project_notice: fileDescriptor(candidateRoot, "NOTICE"),
    rust_packages: cargo.packages.length,
    npm_packages: npm.packages.length,
    packages_total: packageCount,
    missing_license_expression: [
      ...cargo.packages.filter((pkg) => !pkg.license_expression).map((pkg) => ({ ecosystem: "cargo", package: pkg.identity })),
      ...npm.packages.filter((pkg) => !pkg.license_expression).map((pkg) => ({ ecosystem: "npm", package: pkg.package_path })),
    ],
    missing_local_license_text: [
      ...cargo.packages.filter((pkg) => pkg.license_texts.length === 0).map((pkg) => ({ ecosystem: "cargo", package: pkg.identity })),
      ...npm.packages.filter((pkg) => pkg.license_texts.length === 0).map((pkg) => ({ ecosystem: "npm", package: pkg.package_path })),
    ],
    review_boundary: "The report covers the locked Cargo workspace closure and npm production/runtime entries. Expressions come from package metadata; texts come from installed packages, the workspace license, or exact-commit audited evidence.",
  };
  licenseReport.redistribution_ready = licenseReport.missing_license_expression.length === 0
    && licenseReport.missing_local_license_text.length === 0;
  writeJson(join(candidateRoot, "sbom", "license-report.json"), licenseReport);
  const components = [
    ...cargo.packages.map((pkg) => ({
      type: "library",
      "bom-ref": `cargo:${pkg.identity}`,
      name: pkg.name,
      version: pkg.version,
      licenses: pkg.license_expression ? [{ expression: pkg.license_expression }] : undefined,
    })),
    ...npm.packages.map((pkg) => ({
      type: "library",
      "bom-ref": `npm:${pkg.package_path}`,
      name: pkg.name,
      version: pkg.version,
      licenses: pkg.license_expression ? [{ expression: pkg.license_expression }] : undefined,
    })),
  ].map((component) => Object.fromEntries(Object.entries(component).filter(([, value]) => value !== undefined)))
    .sort((left, right) => left["bom-ref"].localeCompare(right["bom-ref"]));
  writeJson(join(candidateRoot, "sbom", "cyclonedx-1.5.json"), {
    bomFormat: "CycloneDX",
    specVersion: "1.5",
    version: 1,
    metadata: { component: { type: "application", name: "sr-doctor", version: productVersion } },
    components,
  });
  return { licenseReport, productVersion };
}

function toolVersions(sourceRoot) {
  return {
    cargo: commandText("cargo", ["--version"], sourceRoot),
    node: commandText("node", ["--version"], sourceRoot),
    npm: commandText("npm", ["--version"], sourceRoot),
    rustc: commandText("rustc", ["--version"], sourceRoot),
  };
}

export function defaultRunId(now = new Date(), sourceRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..")) {
  const productVersion = productVersionFromSourceRoot(sourceRoot);
  return `v${productReleaseLine(productVersion)}-${now.toISOString().replace(/[-:]/g, "").replace(/\.\d{3}Z$/, "Z").toLowerCase()}`;
}

export function assertRunId(runId) {
  if (!/^[a-z0-9][a-z0-9._-]{2,79}$/.test(runId)) fail(`run id must match [a-z0-9][a-z0-9._-]{2,79}: ${runId}`);
}

export function assertFormalBuildSource(provenance, skipBuild) {
  if (!skipBuild && provenance.dirty) {
    fail("formal Candidate release build requires a clean Git source tree; --skip-build remains mechanical repackage evidence only");
  }
  if (!skipBuild && !provenance.git_detached) {
    fail("formal Candidate release build requires a clean detached Git source tree");
  }
}

export function assertCandidateBuildIdentity(candidate) {
  if (typeof candidate.build?.executed !== "boolean") fail("candidate manifest build mode is missing");
  if (candidate.build.executed) {
    if (candidate.build.embedded_identity?.git_commit !== candidate.source?.git_head
      || candidate.build.embedded_identity?.candidate_label !== candidate.run_id) {
      fail("Candidate embedded build identity does not match its frozen source and run id");
    }
  } else if (candidate.build.embedded_identity !== null) {
    fail("prebuilt Candidate must not claim a verified embedded build identity");
  }
}

export function runReleaseBuild(sourceRoot, identity) {
  command("npm", ["--prefix", "frontend", "ci", "--ignore-scripts"], sourceRoot);
  command("cargo", ["build", "--release", "--locked", "--package", "doctor-api"], sourceRoot, null, {
    DOCTOR_BUILD_GIT_COMMIT: identity.git_commit,
    DOCTOR_CANDIDATE_LABEL: identity.candidate_label,
  });
  command("npm", ["--prefix", "frontend", "run", "build"], sourceRoot, null);
}

export function createCandidate(options) {
  const sourceRoot = resolve(options.sourceRoot);
  const outputRoot = resolve(options.outputRoot ?? process.env.DOCTOR_RELEASE_ROOT ?? DEFAULT_RELEASE_ROOT);
  ensureExistingDirectory(sourceRoot, "source root");
  const runId = options.runId ?? defaultRunId(new Date(), sourceRoot);
  assertRunId(runId);
  ensureDirectory(outputRoot);
  const finalRoot = join(outputRoot, runId);
  if (existsSync(finalRoot)) fail(`Candidate output already exists and will not be overwritten: ${finalRoot}`);

  const before = gitProvenance(sourceRoot);
  assertFormalBuildSource(before, Boolean(options.skipBuild));
  const embeddedIdentity = options.skipBuild ? null : {
    git_commit: before.git_head,
    candidate_label: runId,
  };
  const requestedCommands = [
    "npm --prefix frontend ci --ignore-scripts",
    "cargo build --release --locked --package doctor-api",
    "npm --prefix frontend run build",
  ];
  if (!options.skipBuild) runReleaseBuild(sourceRoot, embeddedIdentity);
  if (options.skipBuild && (!options.binaryPath || !options.frontendDist)) {
    fail("--skip-build requires explicit binaryPath and frontendDist");
  }
  const after = gitProvenance(sourceRoot);
  if (before.git_head !== after.git_head || before.dirty_source_snapshot_sha256 !== after.dirty_source_snapshot_sha256) {
    fail("source changed while Candidate was built; retry from a stable source snapshot");
  }

  const binaryPath = resolve(options.binaryPath ?? join(sourceRoot, "target", "release", "doctor-api.exe"));
  const frontendDist = resolve(options.frontendDist ?? join(sourceRoot, "frontend", "dist"));
  ensureExistingFile(binaryPath, "doctor-api release binary");
  ensureExistingDirectory(frontendDist, "frontend dist");
  const staging = join(outputRoot, `.${runId}.staging-${process.pid}-${Date.now()}`);
  mkdirSync(staging, { recursive: false });
  const candidateRoot = join(staging, "candidate");
  mkdirSync(candidateRoot, { recursive: false });
  try {
    copyInto(binaryPath, candidateRoot, "doctor-api.exe");
    copyFrontend(frontendDist, candidateRoot);
    writeStartScript(candidateRoot);
    copyInto(join(sourceRoot, "LICENSE"), candidateRoot, "LICENSE");
    copyInto(join(sourceRoot, "NOTICE"), candidateRoot, "NOTICE");
    const { licenseReport, productVersion } = writeLicenseArtifacts(candidateRoot, sourceRoot);
    if (!licenseReport.redistribution_ready) {
      fail(`Candidate license gate failed: ${licenseReport.missing_license_expression.length} missing expressions, ${licenseReport.missing_local_license_text.length} missing texts`);
    }
    writeGeneratedDocs(candidateRoot, licenseReport, productVersion);
    const locks = lockDescriptors(sourceRoot);
    const finalSource = gitProvenance(sourceRoot);
    if (after.git_head !== finalSource.git_head || after.dirty_source_snapshot_sha256 !== finalSource.dirty_source_snapshot_sha256) {
      fail("source changed while Candidate was being packaged; retry from a stable source snapshot");
    }
    const candidateManifest = {
      schema_version: CANDIDATE_SCHEMA,
      run_id: runId,
      product_version: productVersion,
      created_at_utc: new Date().toISOString(),
      source: finalSource,
      locks,
      build: {
        executed: !options.skipBuild,
        embedded_identity: embeddedIdentity,
        requested_commands: requestedCommands,
        tool_versions: toolVersions(sourceRoot),
      },
      entrypoints: {
        api_binary: "doctor-api.exe",
        frontend_dist: "frontend",
        start_script: "start.ps1",
        bind_address: "127.0.0.1:8000",
      },
      license_review: {
        artifact: "sbom/license-report.json",
        missing_license_expression_count: licenseReport.missing_license_expression.length,
        missing_local_license_text_count: licenseReport.missing_local_license_text.length,
        status: "complete",
        redistribution_status: "ready_with_included_license_and_notices",
      },
      package_files: packageFiles(candidateRoot),
      self_hash_excluded: true,
    };
    writeJson(join(candidateRoot, "candidate-manifest.json"), candidateManifest);
    assertSafeCandidateContents(candidateRoot);
    const archiveName = `sr-doctor-${runId}.zip`;
    const archivePath = join(staging, archiveName);
    createZip(candidateRoot, archivePath);
    const releaseManifest = {
      schema_version: CANDIDATE_SCHEMA,
      run_id: runId,
      product_version: productVersion,
      candidate_directory: "candidate",
      candidate_manifest: fileDescriptor(candidateRoot, "candidate-manifest.json"),
      archive: { file: archiveName, size_bytes: statSync(archivePath).size, sha256: sha256File(archivePath) },
      entrypoints: {
        api_binary: "candidate/doctor-api.exe",
        frontend_dist: "candidate/frontend",
        start_script: "candidate/start.ps1",
        bind_address: "127.0.0.1:8000",
      },
    };
    writeJson(join(staging, "release-manifest.json"), releaseManifest);
    verifyCandidate(staging);
    renameSync(staging, finalRoot);
    return {
      run_root: finalRoot,
      api_binary: join(finalRoot, "candidate", "doctor-api.exe"),
      frontend_dist: join(finalRoot, "candidate", "frontend"),
      start_script: join(finalRoot, "candidate", "start.ps1"),
      archive: join(finalRoot, archiveName),
      release_manifest: join(finalRoot, "release-manifest.json"),
    };
  } catch (error) {
    writeFileSync(join(staging, "FAILED.txt"), `${error instanceof Error ? error.message : String(error)}\n`, "utf8");
    throw error;
  }
}

function candidateSbomProductEvidence(candidateRoot) {
  const cyclonedx = JSON.parse(readFileSync(join(candidateRoot, "sbom", "cyclonedx-1.5.json"), "utf8"));
  if (cyclonedx?.metadata?.component?.type !== "application" || cyclonedx?.metadata?.component?.name !== "sr-doctor") {
    fail("CycloneDX application component does not identify sr-doctor");
  }
  const cargo = JSON.parse(readFileSync(join(candidateRoot, "sbom", "cargo-dependencies.json"), "utf8"));
  const doctorApi = (cargo.packages ?? []).filter((pkg) => pkg.name === "doctor-api" && pkg.source === "workspace");
  if (doctorApi.length !== 1) fail("Cargo SBOM must contain exactly one workspace doctor-api package");
  const npm = JSON.parse(readFileSync(join(candidateRoot, "sbom", "npm-dependencies.json"), "utf8"));
  return {
    cyclonedxVersion: packageVersion(cyclonedx.metadata.component, "CycloneDX application component"),
    cargoProductVersion: packageVersion(doctorApi[0], "Cargo SBOM doctor-api"),
    npmProductVersion: packageVersion(npm.project, "npm SBOM project"),
  };
}

// Pre-0.2 artifacts did not carry manifest product_version. They remain
// verifiable only when all embedded SBOM evidence proves the known v0.1.0
// product identity; absence must never downgrade a v0.2 artifact.
export function assertCandidateProductVersionEvidence({ candidateProductVersion, releaseProductVersion, cyclonedxVersion, cargoProductVersion, npmProductVersion }) {
  const evidence = [
    ["CycloneDX application", cyclonedxVersion],
    ["Cargo SBOM doctor-api", cargoProductVersion],
    ["npm SBOM project", npmProductVersion],
  ];
  if (candidateProductVersion === undefined && releaseProductVersion === undefined) {
    if (evidence.every(([, version]) => version === "0.1.0")) return "0.1.0";
    fail("legacy Candidate is missing product_version without consistent v0.1.0 SBOM evidence");
  }
  if (typeof candidateProductVersion !== "string" || typeof releaseProductVersion !== "string"
    || !candidateProductVersion || !releaseProductVersion) {
    fail("candidate and release manifests must both declare a non-empty product_version");
  }
  if (candidateProductVersion !== releaseProductVersion) {
    fail("candidate and release manifest product_version values disagree");
  }
  const mismatch = evidence.find(([, version]) => version !== candidateProductVersion);
  if (mismatch) {
    fail(`${mismatch[0]} product version ${mismatch[1]} disagrees with candidate manifest ${candidateProductVersion}`);
  }
  return candidateProductVersion;
}

export function verifyCandidate(runRoot) {
  const root = resolve(runRoot);
  const releaseManifestPath = join(root, "release-manifest.json");
  ensureExistingFile(releaseManifestPath, "release manifest");
  const release = JSON.parse(readFileSync(releaseManifestPath, "utf8"));
  if (release.schema_version !== CANDIDATE_SCHEMA) fail(`unexpected candidate schema: ${release.schema_version}`);
  assertRunId(release.run_id);
  if (release.candidate_directory !== "candidate") fail("candidate directory must be exactly candidate");
  const expectedArchiveName = `sr-doctor-${release.run_id}.zip`;
  if (release.archive?.file !== expectedArchiveName) fail("Candidate archive name does not match the immutable run id");
  const candidateRoot = join(root, "candidate");
  ensureExistingDirectory(candidateRoot, "candidate directory");
  const actualCandidateManifest = fileDescriptor(candidateRoot, "candidate-manifest.json");
  if (actualCandidateManifest.sha256 !== release.candidate_manifest.sha256 || actualCandidateManifest.size_bytes !== release.candidate_manifest.size_bytes) {
    fail("candidate manifest hash or size does not match release manifest");
  }
  const candidate = JSON.parse(readFileSync(join(candidateRoot, "candidate-manifest.json"), "utf8"));
  if (candidate.schema_version !== CANDIDATE_SCHEMA || candidate.run_id !== release.run_id) fail("candidate manifest identity mismatch");
  const productVersion = assertCandidateProductVersionEvidence({
    candidateProductVersion: candidate.product_version,
    releaseProductVersion: release.product_version,
    ...candidateSbomProductEvidence(candidateRoot),
  });
  if (candidate.license_review?.status !== "complete"
    || candidate.license_review?.redistribution_status !== "ready_with_included_license_and_notices"
    || candidate.license_review?.missing_license_expression_count !== 0
    || candidate.license_review?.missing_local_license_text_count !== 0) {
    fail("candidate license gate is incomplete");
  }
  const expectedEntrypoints = {
    api_binary: "doctor-api.exe",
    frontend_dist: "frontend",
    start_script: "start.ps1",
    bind_address: "127.0.0.1:8000",
  };
  if (!candidate.entrypoints
    || Object.keys(candidate.entrypoints).length !== Object.keys(expectedEntrypoints).length
    || Object.entries(expectedEntrypoints).some(([key, value]) => candidate.entrypoints[key] !== value)) {
    fail("candidate entrypoints do not match the fixed loopback package contract");
  }
  const expectedReleaseEntrypoints = {
    api_binary: "candidate/doctor-api.exe",
    frontend_dist: "candidate/frontend",
    start_script: "candidate/start.ps1",
    bind_address: "127.0.0.1:8000",
  };
  if (!release.entrypoints
    || Object.keys(release.entrypoints).length !== Object.keys(expectedReleaseEntrypoints).length
    || Object.entries(expectedReleaseEntrypoints).some(([key, value]) => release.entrypoints[key] !== value)) {
    fail("release entrypoints do not match the fixed loopback package contract");
  }
  assertCandidateBuildIdentity(candidate);
  const expected = new Map(candidate.package_files.map((entry) => [entry.path, entry]));
  const actual = listRegularFiles(candidateRoot).map((file) => posixPath(relative(candidateRoot, file))).filter((path) => path !== "candidate-manifest.json");
  if (actual.length !== expected.size || actual.some((path) => !expected.has(path))) fail("candidate files do not exactly match the manifest allowlist");
  for (const [path, descriptor] of expected) {
    assertCandidatePath(path);
    const actualDescriptor = fileDescriptor(candidateRoot, path);
    if (actualDescriptor.size_bytes !== descriptor.size_bytes || actualDescriptor.sha256 !== descriptor.sha256) fail(`candidate file hash mismatch: ${path}`);
  }
  assertSafeCandidateContents(candidateRoot);
  const archivePath = join(root, expectedArchiveName);
  ensureExistingFile(archivePath, "Candidate archive");
  if (statSync(archivePath).size !== release.archive.size_bytes || sha256File(archivePath) !== release.archive.sha256) fail("Candidate archive hash mismatch");
  const expectedZip = expectedZipEntries(candidateRoot);
  const expectedZipDescriptors = new Map(listRegularFiles(candidateRoot).map((file) => {
    const descriptor = fileDescriptor(candidateRoot, posixPath(relative(candidateRoot, file)));
    return [`candidate/${descriptor.path}`, descriptor];
  }));
  const maxUncompressedBytes = [...expectedZipDescriptors.values()].reduce((sum, entry) => sum + entry.size_bytes, 0);
  if (!Number.isSafeInteger(maxUncompressedBytes)) fail("candidate package size exceeds safe integer range");
  const actualZipDescriptors = zipEntryDescriptors(archivePath, { maxEntries: expectedZip.length, maxUncompressedBytes });
  const actualZip = actualZipDescriptors.map((entry) => entry.path).sort();
  if (expectedZip.length !== actualZip.length || expectedZip.some((entry, index) => entry !== actualZip[index])) {
    fail(`Candidate archive entries do not match the explicit package allowlist (expected ${expectedZip.length}, actual ${actualZip.length}; first expected=${expectedZip.slice(0, 6).join(",")}; first actual=${actualZip.slice(0, 6).join(",")})`);
  }
  for (const entry of actualZipDescriptors) {
    const descriptor = expectedZipDescriptors.get(entry.path);
    if (!descriptor || descriptor.size_bytes !== entry.size_bytes || descriptor.sha256 !== entry.sha256) {
      fail(`Candidate archive entry hash mismatch: ${entry.path}`);
    }
  }
  return { run_root: root, archive: archivePath, package_file_count: expected.size, product_version: productVersion };
}
