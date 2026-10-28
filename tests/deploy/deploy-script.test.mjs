import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

// The one-click deployment (root compose.yaml and deploy.sh): how the login
// account is set, and that the script's argument checks fail before any Docker
// call. Real installs are verified by hand against Docker (see the
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

test("the login defaults to admin and settings stay out of Git and the image", () => {
  // Empty values fall back to admin / 123456 in the server.
  assert.match(compose, /DOCTOR_ADMIN_USER: "\$\{DOCTOR_ADMIN_USER:-\}"/);
  assert.match(compose, /DOCTOR_ADMIN_PASSWORD: "\$\{DOCTOR_ADMIN_PASSWORD:-\}"/);
  assert.doesNotMatch(compose, /token-init|doctor-secrets|ACCESS_TOKEN/);
  assert.match(script, /^VOLUME="sr-doctor_doctor-history"$/m);
  assert.match(script, /\[ -n "\$OPT_PASSWORD" \] && env_set DOCTOR_ADMIN_PASSWORD "\$OPT_PASSWORD"/);
  for (const entry of ["/.env", "/compose.override.yaml", "deploy/linux/access-token", "deploy/linux/.env", "/dist/"]) assert.ok(gitignore.includes(entry), entry);
  for (const entry of ["deploy/**/access-token", ".env", "compose.override.yaml"]) assert.ok(dockerignore.includes(entry), entry);
});

test("a password set in .env is never printed", () => {
  const print = script.slice(script.indexOf("print_access() {"), script.indexOf("\n}\n", script.indexOf("print_access() {")));
  for (const line of print.split("\n").filter((line) => line.includes("printf"))) assert.doesNotMatch(line, /ADMIN_PASSWORD/, line);
  assert.ok(print.includes(`if [ -z "$(env_get DOCTOR_ADMIN_PASSWORD "")" ]; then`));
  assert.ok(print.includes("（密码已在 .env 中设置）"));
});

test("destructive commands ask first", () => {
  for (const fn of ["cmd_restore"]) {
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
  for (const args of [["install", "--port", "70000"], ["install", "--port", "8o00"], ["install", "--bind", "1.2.3.4;id"], ["--password", "has space"], ["--password", "a$b"], ["--password", "a#b"], ["--bogus"], ["nonsense"], ["token"]]) {
    const result = run(...args);
    assert.equal(result.status, 1, `${args.join(" ")}: ${result.stdout}${result.stderr}`);
    assert.match(result.stderr, /错误：/);
  }
});
