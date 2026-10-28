import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const script = resolve(dirname(fileURLToPath(import.meta.url)), "windows-clean-host-smoke.ps1");
const commit = "0123456789abcdef0123456789abcdef01234567";

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function descriptor(root, relative) {
  const path = join(root, ...relative.split("/"));
  return { path: relative, size_bytes: statSync(path).size, sha256: sha256(path) };
}

function compress(candidate, archive) {
  const command = `Compress-Archive -LiteralPath '${candidate.replaceAll("'", "''")}' -DestinationPath '${archive.replaceAll("'", "''")}' -Force`;
  const result = spawnSync("powershell.exe", ["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", command], {
    encoding: "utf8",
    windowsHide: true,
  });
  assert.equal(result.status, 0, result.stderr);
}

function runSmoke({ archive, expectedHash, workRoot, receipt }) {
  return spawnSync("powershell.exe", [
    "-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
    "-File", script,
    "-ZipPath", archive,
    "-ExpectedZipSha256", expectedHash,
    "-ExpectedCommit", commit,
    "-WorkRoot", workRoot,
    "-ReceiptPath", receipt,
    "-VerifyOnly",
  ], { encoding: "utf8", windowsHide: true });
}

test("Windows clean-host verifier accepts an intact formal package and fails closed on hash or payload tampering", { skip: process.platform !== "win32" }, () => {
  const temporary = mkdtempSync(join(tmpdir(), "windows-clean-host-smoke-"));
  try {
    const source = join(temporary, "source");
    const candidate = join(source, "candidate");
    mkdirSync(join(candidate, "frontend"), { recursive: true });
    mkdirSync(join(candidate, "sbom"), { recursive: true });
    writeFileSync(join(candidate, "doctor-api.exe"), "fixture binary\n");
    writeFileSync(join(candidate, "frontend", "index.html"), "<!doctype html><title>fixture</title>\n");
    writeFileSync(join(candidate, "start.ps1"), "Write-Host 'fixture'\n");
    writeFileSync(join(candidate, "sbom", "cyclonedx-1.5.json"), `${JSON.stringify({
      bomFormat: "CycloneDX",
      specVersion: "1.5",
      metadata: { component: { type: "application", name: "sr-doctor", version: "0.2.0" } },
    })}\n`);
    writeFileSync(join(candidate, "sbom", "cargo-dependencies.json"), `${JSON.stringify({
      packages: [{ name: "doctor-api", version: "0.2.0", source: "workspace" }],
    })}\n`);
    writeFileSync(join(candidate, "sbom", "npm-dependencies.json"), `${JSON.stringify({
      project: { name: "sr-doctor-frontend", version: "0.2.0" },
    })}\n`);
    const packageFiles = [
      "doctor-api.exe",
      "frontend/index.html",
      "start.ps1",
      "sbom/cyclonedx-1.5.json",
      "sbom/cargo-dependencies.json",
      "sbom/npm-dependencies.json",
    ].map((path) => descriptor(candidate, path));
    const manifest = {
      schema_version: "sr-doctor.candidate/v0.1",
      run_id: "fixture-windows-clean-host",
      product_version: "0.2.0",
      source: { dirty: false, dirty_entry_count: 0, git_head: commit },
      build: { executed: true, embedded_identity: { git_commit: commit, candidate_label: "fixture-windows-clean-host" } },
      entrypoints: { api_binary: "doctor-api.exe", frontend_dist: "frontend", start_script: "start.ps1", bind_address: "127.0.0.1:8000" },
      license_review: {
        status: "complete",
        redistribution_status: "ready_with_included_license_and_notices",
        missing_license_expression_count: 0,
        missing_local_license_text_count: 0,
      },
      package_files: packageFiles,
      self_hash_excluded: true,
    };
    writeFileSync(join(candidate, "candidate-manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);

    const archive = join(temporary, "candidate.zip");
    compress(candidate, archive);
    const goodReceipt = join(temporary, "good-receipt.json");
    const good = runSmoke({ archive, expectedHash: sha256(archive), workRoot: join(temporary, "good-work"), receipt: goodReceipt });
    assert.equal(good.status, 0, `${good.stdout}\n${good.stderr}`);
    const goodResult = JSON.parse(readFileSync(goodReceipt, "utf8"));
    assert.equal(goodResult.status, "PASS");
    assert.equal(goodResult.candidate.package_file_count, 6);
    assert.equal(goodResult.candidate.product_version, "0.2.0");
    assert.equal(goodResult.candidate.product_version_evidence, "manifest_and_sbom");

    const badHashReceipt = join(temporary, "bad-hash-receipt.json");
    const badHashWork = join(temporary, "bad-hash-work");
    const badHash = runSmoke({ archive, expectedHash: "0".repeat(64), workRoot: badHashWork, receipt: badHashReceipt });
    assert.notEqual(badHash.status, 0);
    assert.equal(JSON.parse(readFileSync(badHashReceipt, "utf8")).failed_stage, "input_preconditions");
    assert.equal(existsSync(badHashWork), false);

    // Legacy means an absent field, not a present null/empty value. Every
    // case carries recomputed payload hashes so the version gate is exercised.
    const sbomFiles = ["cyclonedx-1.5.json", "cargo-dependencies.json", "npm-dependencies.json"];
    const originalSboms = sbomFiles.map((file) => readFileSync(join(candidate, "sbom", file), "utf8"));
    const versionCases = [
      ["legacy-absent", "0.1.0", undefined, true],
      ["legacy-null", "0.1.0", null, false],
      ["legacy-empty", "0.1.0", "", false],
      ["current-absent", "0.2.0", undefined, false],
      ["current-mismatch", "0.2.0", "0.1.0", false],
    ];
    for (const [label, sbomVersion, manifestVersion, allowed] of versionCases) {
      sbomFiles.forEach((file, i) => writeFileSync(join(candidate, "sbom", file), originalSboms[i].replaceAll("0.2.0", sbomVersion)));
      const versionManifest = { ...manifest, product_version: manifestVersion,
        package_files: packageFiles.map(({ path }) => descriptor(candidate, path)) };
      writeFileSync(join(candidate, "candidate-manifest.json"), `${JSON.stringify(versionManifest)}\n`);
      compress(candidate, archive);
      const receipt = join(temporary, `${label}-receipt.json`);
      const result = runSmoke({ archive, expectedHash: sha256(archive), workRoot: join(temporary, `${label}-work`), receipt });
      assert.equal(result.status === 0, allowed, `${label}: ${result.stdout}\n${result.stderr}`);
      const evidence = JSON.parse(readFileSync(receipt, "utf8"));
      if (allowed) assert.equal(evidence.candidate.product_version_evidence, "legacy_sbom_v0.1.0");
      else assert.match(evidence.error, /product_version/);
    }
    sbomFiles.forEach((file, i) => writeFileSync(join(candidate, "sbom", file), originalSboms[i]));
    writeFileSync(join(candidate, "candidate-manifest.json"), `${JSON.stringify(manifest)}\n`);

    writeFileSync(join(candidate, "frontend", "index.html"), "tampered\n");
    compress(candidate, archive);
    const tamperedReceipt = join(temporary, "tampered-receipt.json");
    const tampered = runSmoke({ archive, expectedHash: sha256(archive), workRoot: join(temporary, "tampered-work"), receipt: tamperedReceipt });
    assert.notEqual(tampered.status, 0);
    assert.equal(JSON.parse(readFileSync(tamperedReceipt, "utf8")).failed_stage, "payload_hashes");
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
});
