import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

// The one-click deployment (root compose.yaml and deploy.sh): what they may do
// with the access token, and that the script's argument checks fail before any
// Docker call. Real installs are verified by hand against Docker (see the
// task 33 receipt).
const script = readFileSync(resolve("deploy.sh"), "utf8");
const compose = readFileSync(resolve("compose.yaml"), "utf8");
const gitignore = readFileSync(resolve(".gitignore"), "utf8").split(/\r?\n/);
const dockerignore = readFileSync(resolve(".dockerignore"), "utf8").split(/\r?\n/);

function findBash() {
  if (process.platform !== "win32") return "bash";
  // On Windows `bash` may be the WSL launcher; use the one from Git for Windows.
  try {
    const execPath = execFileSync("git", ["--exec-path"], { encoding: "utf8" }).trim();
    const bash = resolve(execPath, "../../../bin/bash.exe");
    return existsSync(bash) ? bash : null;
  } catch {
    return null;
  }
}

test("the script stops on the first error and never traces commands", () => {
  assert.match(script, /^set -euo pipefail$/m);
  assert.doesNotMatch(script, /set -x|set -o xtrace|bash -x/);
});

test("the token lives in its own volume and settings stay out of Git and the image", () => {
  assert.match(compose, /^\s+- doctor-secrets:\/etc\/sr-doctor$/m);
  assert.match(compose, /^\s+- doctor-secrets:\/etc\/sr-doctor:ro$/m);
  assert.match(compose, /DOCTOR_ACCESS_TOKEN_FILE: \/etc\/sr-doctor\/access-token/);
  // Backups tar the history volume only, so they never carry the token.
  assert.match(script, /^VOLUME="sr-doctor_doctor-history"$/m);
  assert.match(script, /^SECRETS_VOLUME="sr-doctor_doctor-secrets"$/m);
  assert.doesNotMatch(script.slice(script.indexOf("cmd_backup() {"), script.indexOf("cmd_restore() {")), /SECRETS_VOLUME/);
  for (const entry of ["/.env", "/compose.override.yaml", "deploy/linux/access-token", "deploy/linux/.env", "/dist/"]) assert.ok(gitignore.includes(entry), entry);
  for (const entry of ["deploy/**/access-token", ".env", "compose.override.yaml"]) assert.ok(dockerignore.includes(entry), entry);
});

test("token-init writes the token once, privately, and never prints it", () => {
  const init = compose.slice(compose.indexOf("  token-init:"), compose.indexOf("  doctor:"));
  assert.match(init, /network_mode: none/);
  assert.match(init, /umask 077/);
  assert.ok(init.includes(`if [ "$\${DOCTOR_TOKEN_ROTATE:-0}" != 1 ] && [ "$\${#current}" -ge 16 ]; then exit 0; fi`));
  assert.doesNotMatch(init, /echo|set -x|\bcat /);
  assert.match(compose, /depends_on:\n\s+token-init:\n\s+condition: service_completed_successfully/);
});

test("the token is printed only when newly generated or asked for", () => {
  const uses = script.split("\n").filter((line) => line.includes("$(read_token)"));
  // One line in print_access (behind NEW_TOKEN) and one in cmd_token.
  assert.equal(uses.length, 2, uses.join("\n"));
  assert.ok(script.includes(`if [ "$NEW_TOKEN" = 1 ]; then\n    printf '  访问令牌：%s\\n' "$(read_token)"`));
  assert.match(script, /cmd_token\(\) \{\n[^}]*token="\$\(read_token\)"/);
});

test("destructive commands ask first", () => {
  for (const fn of ["cmd_rotate_token", "cmd_restore"]) {
    const body = script.slice(script.indexOf(`${fn}() {`), script.indexOf("\n}\n", script.indexOf(`${fn}() {`)));
    assert.match(body, /confirm "/, fn);
  }
  const uninstall = script.slice(script.indexOf("cmd_uninstall() {"));
  assert.match(uninstall, /if \[ "\$OPT_PURGE" = 1 \]; then\n\s+confirm "/);
});

const bash = findBash();

test("help and argument checks run without Docker", { skip: bash ? false : "no POSIX bash found" }, () => {
  const run = (...args) => spawnSync(bash, ["deploy.sh", ...args], { encoding: "utf8", env: { ...process.env, PATH: process.env.PATH } });
  const syntax = spawnSync(bash, ["-n", "deploy.sh"], { encoding: "utf8" });
  assert.equal(syntax.status, 0, syntax.stderr);
  const help = run("help");
  assert.equal(help.status, 0, help.stderr);
  assert.match(help.stdout, /install/);
  assert.match(help.stdout, /package/);
  for (const args of [["install", "--port", "70000"], ["install", "--port", "8o00"], ["install", "--bind", "1.2.3.4;id"], ["--bogus"], ["nonsense"]]) {
    const result = run(...args);
    assert.equal(result.status, 1, `${args.join(" ")}: ${result.stdout}${result.stderr}`);
    assert.match(result.stderr, /错误：/);
  }
});
