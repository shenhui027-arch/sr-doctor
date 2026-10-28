#!/usr/bin/env node
import { verifyCandidate } from "./candidate-lib.mjs";

function usage() {
  return "Usage: node scripts/release/verify-candidate.mjs --release <RUN_ROOT>\n";
}

try {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === "--help") {
    process.stdout.write(usage());
    process.exit(0);
  }
  if (args.length !== 2 || args[0] !== "--release" || !args[1]) throw new Error("--release <RUN_ROOT> is required");
  const result = verifyCandidate(args[1]);
  process.stdout.write(`VERIFIED_RUN_ROOT=${result.run_root}\nPACKAGE_FILE_COUNT=${result.package_file_count}\nARCHIVE=${result.archive}\n`);
} catch (error) {
  process.stderr.write(`Candidate verification failed: ${error instanceof Error ? error.message : String(error)}\n`);
  process.exitCode = 1;
}
