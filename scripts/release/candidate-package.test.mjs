import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";
import {
  assertCandidateBuildIdentity,
  assertCandidateProductVersionEvidence,
  assertFormalBuildSource,
  candidatePathAllowed,
  createCandidate,
  defaultRunId,
  generatedSupportDocument,
  generatedSecurityDocument,
  productVersionFromSourceMetadata,
  verifyCandidate,
  zipEntries,
} from "./candidate-lib.mjs";

const sourceRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function replaceZipEntry(archive, entryName, contents) {
  const script = [
    "Add-Type -AssemblyName System.IO.Compression",
    "Add-Type -AssemblyName System.IO.Compression.FileSystem",
    `$zip = [System.IO.Compression.ZipFile]::Open('${archive.replace(/'/g, "''")}', [System.IO.Compression.ZipArchiveMode]::Update)`,
    "try {",
    `  $entry = $zip.Entries | Where-Object { $_.FullName.Replace('\\', '/') -eq '${entryName.replace(/'/g, "''")}' } | Select-Object -First 1`,
    "  if ($null -eq $entry) { throw 'fixture entry missing' }",
    "  $entry.Delete()",
    `  $replacement = $zip.CreateEntry('${entryName.replace(/'/g, "''")}')`,
    "  $writer = [System.IO.StreamWriter]::new($replacement.Open())",
    `  try { $writer.Write('${contents.replace(/'/g, "''")}') } finally { $writer.Dispose() }`,
    "} finally { $zip.Dispose() }",
  ].join("; ");
  const result = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script], {
    encoding: "utf8",
    windowsHide: true,
  });
  assert.equal(result.status, 0, result.stderr);
}

function fixtureBuild(root) {
  const binary = join(root, "prebuilt", "doctor-api.exe");
  const dist = join(root, "prebuilt", "frontend-dist");
  mkdirSync(dirname(binary), { recursive: true });
  mkdirSync(join(dist, "assets"), { recursive: true });
  mkdirSync(join(dist, "licenses"), { recursive: true });
  writeFileSync(binary, "fixture doctor api binary\n", "utf8");
  writeFileSync(join(dist, "index.html"), "<!doctype html><script src=\"/assets/app.js\"></script>", "utf8");
  writeFileSync(join(dist, "assets", "app.js"), "window.fixture = true; const inputTypes = { password: !0, range: !0 };", "utf8");
  writeFileSync(join(dist, "assets", "app.css"), "body { color: #123; }", "utf8");
  writeFileSync(join(dist, "assets", "font.woff2"), "not-a-real-font", "utf8");
  writeFileSync(join(dist, "assets", "status.svg"), "<svg></svg>", "utf8");
  writeFileSync(join(dist, "licenses", "Inter-OFL-1.1.txt"), "OFL", "utf8");
  writeFileSync(join(dist, "licenses", "JetBrains-Mono-OFL-1.1.txt"), "OFL", "utf8");
  return { binary, dist };
}

test("Candidate package uses a strict allowlist, hashes every payload file, and rejects tampering", () => {
  const temporary = mkdtempSync(join(tmpdir(), "sr-doctor-candidate-"));
  try {
    const fixture = fixtureBuild(temporary);
    const outputRoot = join(temporary, "release");
    const candidate = createCandidate({
      sourceRoot,
      outputRoot,
      runId: "fixture-release-001",
      skipBuild: true,
      binaryPath: fixture.binary,
      frontendDist: fixture.dist,
    });
    const launcher = readFileSync(join(candidate.run_root, "candidate", "start.ps1"), "utf8");
    assert.ok(launcher.includes("[ValidateRange(1, 65535)][int]$Port = 8000"));
    assert.ok(launcher.includes("$env:DOCTOR_PORT = [string]$Port"));
    assert.ok(launcher.includes("http://127.0.0.1:$Port"));
    assert.ok(launcher.includes("$env:DOCTOR_PORT = $previousPort"));
    const verified = verifyCandidate(candidate.run_root);
    assert.ok(verified.package_file_count > 10);
    assert.equal(verified.product_version, "1.0.0");

    const releaseManifest = JSON.parse(readFileSync(candidate.release_manifest, "utf8"));
    const candidateManifest = JSON.parse(readFileSync(join(candidate.run_root, "candidate", "candidate-manifest.json"), "utf8"));
    const listed = candidateManifest.package_files.map((entry) => entry.path);
    assert.ok(listed.includes("LICENSE"));
    assert.ok(listed.includes("NOTICE"));
    assert.ok(listed.includes("doctor-api.exe"));
    assert.ok(listed.includes("frontend/index.html"));
    assert.ok(listed.includes("sbom/cargo-dependencies.json"));
    assert.ok(listed.includes("sbom/npm-dependencies.json"));
    assert.ok(listed.includes("sbom/cyclonedx-1.5.json"));
    assert.deepEqual(listed.filter((path) => path.startsWith("docs/")), [
      "docs/LICENSE-STATUS.md",
      "docs/ROLLBACK.md",
      "docs/SECURITY.md",
      "docs/SUPPORT.md",
      "docs/THIRD-PARTY-NOTICES.md",
    ]);
    assert.equal(listed.some((path) => /northstar|raw-profile|runtime\//i.test(path)), false);
    assert.equal(releaseManifest.entrypoints.bind_address, "127.0.0.1:8000");
    assert.equal(candidateManifest.product_version, "1.0.0");
    assert.equal(releaseManifest.product_version, "1.0.0");
    assert.equal(candidateManifest.build.executed, false);
    assert.equal(candidateManifest.build.embedded_identity, null);
    assert.equal(candidateManifest.license_review.status, "complete");
    assert.equal(candidateManifest.license_review.redistribution_status, "ready_with_included_license_and_notices");
    assert.equal(candidateManifest.license_review.missing_license_expression_count, 0);
    assert.equal(candidateManifest.license_review.missing_local_license_text_count, 0);
    assert.ok(candidateManifest.build.requested_commands.includes("npm --prefix frontend ci --ignore-scripts"));

    const cyclonedx = JSON.parse(readFileSync(join(candidate.run_root, "candidate", "sbom", "cyclonedx-1.5.json"), "utf8"));
    assert.equal(cyclonedx.bomFormat, "CycloneDX");
    assert.equal(cyclonedx.specVersion, "1.5");
    assert.equal(cyclonedx.metadata.component.version, "1.0.0");
    assert.ok(cyclonedx.components.length > 0);
    assert.equal(cyclonedx.components.every((component) => component.name && component.version), true);

    const support = readFileSync(join(candidate.run_root, "candidate", "docs", "SUPPORT.md"), "utf8");
    assert.match(support, /^# v1\.0\.0 Candidate support/);
    // The current product rejects EXPLAIN ANALYZE; the native path belongs to the frozen v0.2 line only.
    assert.doesNotMatch(support, /Native execution/);
    assert.match(support, /Multi-statements, EXPLAIN ANALYZE, DDL, DML, INSERT/);
    for (const required of ["3.5.14", "Unregistered versions", "SHOW PROFILELIST", "get_query_profile", "SHOW PLAN ADVISOR", "TLS verify-full", "Credentials", "budgets"]) {
      assert.ok(support.includes(required), `SUPPORT.md must include ${required}`);
    }
    assert.match(readFileSync(join(candidate.run_root, "candidate", "docs", "SECURITY.md"), "utf8"), /Trust boundary:[\s\S]*Runtime Top[\s\S]*10-second[\s\S]*Production-safe is INCONCLUSIVE/);
    assert.match(readFileSync(join(candidate.run_root, "candidate", "docs", "ROLLBACK.md"), "utf8"), /disconnect[\s\S]*stop[\s\S]*hashes[\s\S]*offline import smoke check/i);
    assert.match(readFileSync(join(candidate.run_root, "candidate", "docs", "LICENSE-STATUS.md"), "utf8"), /Redistribution status: READY[\s\S]*Missing expressions: 0[\s\S]*missing license texts: 0/);
    const licenseReport = JSON.parse(readFileSync(join(candidate.run_root, "candidate", "sbom", "license-report.json"), "utf8"));
    assert.equal(licenseReport.redistribution_ready, true);
    assert.deepEqual(licenseReport.missing_license_expression, []);
    assert.deepEqual(licenseReport.missing_local_license_text, []);
    const npmSbom = JSON.parse(readFileSync(join(candidate.run_root, "candidate", "sbom", "npm-dependencies.json"), "utf8"));
    assert.equal(npmSbom.packages.length, 30);
    assert.equal(npmSbom.packages.every((pkg) => pkg.installed_package_json && pkg.license_expression && pkg.license_texts.length > 0), true);

    const expectedArchiveFiles = [...listed, "candidate-manifest.json"].map((path) => `candidate/${path}`).sort();
    assert.deepEqual(zipEntries(candidate.archive).filter((path) => !path.endsWith("/")), expectedArchiveFiles);

    writeFileSync(candidate.release_manifest, `${JSON.stringify({
      ...releaseManifest,
      archive: { ...releaseManifest.archive, file: "../outside.zip" },
    })}\n`, "utf8");
    assert.throws(() => verifyCandidate(candidate.run_root), /archive name does not match/);
    writeFileSync(candidate.release_manifest, `${JSON.stringify(releaseManifest, null, 2)}\n`, "utf8");

    replaceZipEntry(candidate.archive, "candidate/frontend/index.html", "tampered archive payload");
    const archiveTamperedReleaseManifest = {
      ...releaseManifest,
      archive: {
        ...releaseManifest.archive,
        size_bytes: statSync(candidate.archive).size,
        sha256: sha256File(candidate.archive),
      },
    };
    writeFileSync(candidate.release_manifest, `${JSON.stringify(archiveTamperedReleaseManifest, null, 2)}\n`, "utf8");
    assert.throws(() => verifyCandidate(candidate.run_root), /archive entry hash mismatch: candidate\/frontend\/index\.html/);
    writeFileSync(candidate.release_manifest, `${JSON.stringify(releaseManifest, null, 2)}\n`, "utf8");

    writeFileSync(join(candidate.run_root, "candidate", "frontend", "index.html"), "tampered", "utf8");
    assert.throws(() => verifyCandidate(candidate.run_root), /candidate file hash mismatch: frontend\/index\.html/);

    writeFileSync(join(fixture.dist, "assets", "app.js"), "const api_key=supersecretvalue;", "utf8");
    assert.throws(() => createCandidate({
      sourceRoot,
      outputRoot,
      runId: "fixture-secret-002",
      skipBuild: true,
      binaryPath: fixture.binary,
      frontendDist: fixture.dist,
    }), /credential assignment/);
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
});

test("Candidate product version is derived from doctor-api metadata and rejects package-lock drift", () => {
  const cargoMetadata = {
    workspace_members: ["doctor-api 0.2.0 (path+file:///fixture/api)", "doctor-core 0.2.0 (path+file:///fixture/core)"],
    packages: [
      { id: "doctor-api 0.2.0 (path+file:///fixture/api)", name: "doctor-api", version: "0.2.0" },
      { id: "doctor-core 0.2.0 (path+file:///fixture/core)", name: "doctor-core", version: "0.2.0" },
    ],
  };
  const metadata = {
    cargoMetadata,
    rootPackage: { version: "0.2.0" },
    rootPackageLock: { version: "0.2.0", packages: { "": { version: "0.2.0" } } },
    frontendPackage: { version: "0.2.0" },
    frontendPackageLock: { version: "0.2.0", packages: { "": { version: "0.2.0" } } },
  };
  assert.equal(productVersionFromSourceMetadata(metadata), "0.2.0");
  assert.throws(
    () => productVersionFromSourceMetadata({ ...metadata, frontendPackageLock: { version: "0.2.0", packages: { "": { version: "0.1.0" } } } }),
    /frontend package lockfile top-level and root package versions disagree/,
  );
  assert.throws(
    () => productVersionFromSourceMetadata({ ...metadata, rootPackage: { version: "0.1.0" } }),
    /root package\.json version 0\.1\.0 disagrees with doctor-api 0\.2\.0/,
  );
  assert.throws(
    () => productVersionFromSourceMetadata({ ...metadata, frontendPackage: { version: "0.1.0" } }),
    /frontend package\.json version 0\.1\.0 disagrees with doctor-api 0\.2\.0/,
  );
  assert.match(defaultRunId(new Date("2026-09-05T01:02:03.456Z"), sourceRoot), /^v1\.0-20260905t010203z$/);
});

test("Candidate product-version verifier permits only evidence-proven v0.1 legacy absence", () => {
  const current = {
    candidateProductVersion: "0.2.0",
    releaseProductVersion: "0.2.0",
    cyclonedxVersion: "0.2.0",
    cargoProductVersion: "0.2.0",
    npmProductVersion: "0.2.0",
  };
  assert.equal(assertCandidateProductVersionEvidence(current), "0.2.0");
  assert.equal(assertCandidateProductVersionEvidence({
    ...current,
    candidateProductVersion: undefined,
    releaseProductVersion: undefined,
    cyclonedxVersion: "0.1.0",
    cargoProductVersion: "0.1.0",
    npmProductVersion: "0.1.0",
  }), "0.1.0");
  assert.throws(
    () => assertCandidateProductVersionEvidence({ ...current, candidateProductVersion: undefined }),
    /must both declare a non-empty product_version/,
  );
  assert.throws(
    () => assertCandidateProductVersionEvidence({ ...current, releaseProductVersion: undefined }),
    /must both declare a non-empty product_version/,
  );
  assert.throws(
    () => assertCandidateProductVersionEvidence({ ...current, releaseProductVersion: "0.1.0" }),
    /candidate and release manifest product_version values disagree/,
  );
  assert.throws(
    () => assertCandidateProductVersionEvidence({ ...current, candidateProductVersion: undefined, releaseProductVersion: undefined }),
    /missing product_version without consistent v0\.1\.0 SBOM evidence/,
  );
  assert.throws(
    () => assertCandidateProductVersionEvidence({ ...current, cargoProductVersion: "0.1.0" }),
    /Cargo SBOM doctor-api product version 0\.1\.0 disagrees/,
  );
});

test("Candidate allowlist excludes old reference assets, secrets, raw profile inputs, and runtime data", () => {
  assert.equal(candidatePathAllowed("frontend/assets/legacy-reference.png"), false);
  assert.equal(candidatePathAllowed("fixtures/raw-profile.json"), false);
  assert.equal(candidatePathAllowed("runtime/audit.log"), false);
  assert.equal(candidatePathAllowed("secrets/connection.json"), false);
  assert.equal(candidatePathAllowed("docs/raw-profile.txt"), false);
  assert.equal(candidatePathAllowed("docs/private.md"), false);
  assert.equal(candidatePathAllowed("docs/production-corpus.md"), false);
  assert.equal(candidatePathAllowed("frontend/assets/app.js"), true);
  assert.equal(candidatePathAllowed("docs/SUPPORT.md"), true);
  assert.equal(candidatePathAllowed("LICENSE"), true);
  assert.equal(candidatePathAllowed("NOTICE"), true);
});

test("Candidate CLI rejects arbitrary document injection", () => {
  const result = spawnSync(process.execPath, ["scripts/release/build-candidate.mjs", "--include-doc", "docs/private.md"], {
    cwd: sourceRoot,
    encoding: "utf8",
    windowsHide: true,
  });
  assert.notEqual(result.status, 0);
  assert.match(`${result.stdout}\n${result.stderr}`, /unknown argument: --include-doc/);
});

test("formal builds require a clean source while prebuilt packaging stays explicitly mechanical", () => {
  assert.doesNotThrow(() => assertFormalBuildSource({ dirty: false, git_detached: true }, false));
  assert.doesNotThrow(() => assertFormalBuildSource({ dirty: true }, true));
  assert.throws(
    () => assertFormalBuildSource({ dirty: true }, false),
    /formal Candidate release build requires a clean Git source tree/,
  );
  assert.throws(
    () => assertFormalBuildSource({ dirty: false, git_detached: false }, false),
    /formal Candidate release build requires a clean detached Git source tree/,
  );
});

test("Candidate build identity binds a formal binary to its source commit and run id", () => {
  const formal = {
    run_id: "v0.1-candidate-001",
    source: { git_head: "0123456789abcdef0123456789abcdef01234567" },
    build: {
      executed: true,
      embedded_identity: {
        git_commit: "0123456789abcdef0123456789abcdef01234567",
        candidate_label: "v0.1-candidate-001",
      },
    },
  };
  assert.doesNotThrow(() => assertCandidateBuildIdentity(formal));
  assert.throws(
    () => assertCandidateBuildIdentity({ ...formal, build: { ...formal.build, embedded_identity: { ...formal.build.embedded_identity, candidate_label: "other" } } }),
    /does not match its frozen source and run id/,
  );
  assert.doesNotThrow(() => assertCandidateBuildIdentity({ build: { executed: false, embedded_identity: null } }));
  assert.throws(
    () => assertCandidateBuildIdentity({ build: { executed: false, embedded_identity: formal.build.embedded_identity } }),
    /prebuilt Candidate must not claim/,
  );
});

test("Candidate documentation keeps product version and native capability separate from schema", () => {
  const current = generatedSupportDocument("0.2.0");
  const security = generatedSecurityDocument("0.2.0");
  assert.match(current, /^# v0\.2\.0 Candidate support/);
  assert.match(current, /exactly one EXPLAIN ANALYZE[\s\S]*fixed 10-second[\s\S]*does not first execute a bare SELECT/);
  assert.match(current, /Standard Runtime Top executes its SELECT once and never issues EXPLAIN ANALYZE/);
  assert.match(security, /explicitly confirmed native path[\s\S]*never automatically retry or fall back/);
  assert.match(security, /outside v0\.2\.0 validation/);
  const legacy = generatedSupportDocument("0.1.0");
  assert.match(legacy, /^# v0\.1\.0 Candidate support/);
  assert.match(legacy, /Multi-statements, EXPLAIN ANALYZE, DDL/);
  assert.doesNotMatch(legacy, /Native execution is a separate/);
  assert.match(generatedSecurityDocument("0.1.0"), /Native EXPLAIN ANALYZE execution is not offered/);
});