#!/usr/bin/env node
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createCandidate, defaultRunId } from "./candidate-lib.mjs";

function usage() {
  return `Usage:
  node scripts/release/build-candidate.mjs [options]

Builds doctor-api.exe and frontend/dist unless --skip-build is explicitly used.
The output is a new, non-overwritable run directory under
<SR_DOCTOR_WORK_ROOT, or the system temp directory>/sr-doctor/release by default.

Options:
  --run-id <id>             Explicit immutable Candidate run id.
  --output-root <path>      External release root (defaults to DOCTOR_RELEASE_ROOT, then the default above).
  --skip-build              Package explicit prebuilt paths only (test/repackage use).
  --binary <path>           doctor-api.exe path; required with --skip-build.
  --frontend-dist <path>    frontend dist path; required with --skip-build.
  --help                    Show this help.
`;
}

function parse(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index];
    if (token === "--help") options.help = true;
    else if (token === "--skip-build") options.skipBuild = true;
    else if (["--run-id", "--output-root", "--binary", "--frontend-dist"].includes(token)) {
      const value = argv[index + 1];
      if (!value || value.startsWith("--")) throw new Error(`missing value for ${token}`);
      index += 1;
      if (token === "--run-id") options.runId = value;
      if (token === "--output-root") options.outputRoot = value;
      if (token === "--binary") options.binaryPath = value;
      if (token === "--frontend-dist") options.frontendDist = value;
    } else throw new Error(`unknown argument: ${token}`);
  }
  return options;
}

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const sourceRoot = resolve(scriptDirectory, "..", "..");

try {
  const options = parse(process.argv.slice(2));
  if (options.help) {
    process.stdout.write(usage());
    process.exit(0);
  }
  const candidate = createCandidate({ sourceRoot, runId: options.runId ?? defaultRunId(), ...options });
  process.stdout.write([
    `RUN_ROOT=${candidate.run_root}`,
    `API_BINARY=${candidate.api_binary}`,
    `FRONTEND_DIST=${candidate.frontend_dist}`,
    `START_SCRIPT=${candidate.start_script}`,
    `ARCHIVE=${candidate.archive}`,
    `RELEASE_MANIFEST=${candidate.release_manifest}`,
  ].join("\n") + "\n");
} catch (error) {
  process.stderr.write(`Candidate build failed: ${error instanceof Error ? error.message : String(error)}\n`);
  process.exitCode = 1;
}
