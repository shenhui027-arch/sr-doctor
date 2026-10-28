/**
 * G2 calibration I/O shared by the desensitizers and the verifier: the local
 * pseudonym key, `mysql --batch` TSV, receipts. Receipts carry counts and
 * hashes only, never sample content.
 */
import { createHash, randomBytes } from "node:crypto";
import { existsSync, readFileSync, writeFileSync, mkdirSync } from "node:fs";
import path from "node:path";

export function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

/**
 * The pseudonym key stays on the exporting machine. It makes pseudonyms stable
 * across the audit and Profile runs; it must never be shared with the samples.
 */
export function loadKey(keyFile, { create = false } = {}) {
  if (!keyFile) throw new Error("--key-file is required");
  if (!existsSync(keyFile)) {
    if (!create) throw new Error("key file does not exist; pass --create-key to create one");
    mkdirSync(path.dirname(path.resolve(keyFile)), { recursive: true });
    writeFileSync(keyFile, randomBytes(32), { mode: 0o600 });
  }
  const key = readFileSync(keyFile);
  if (key.length < 32) throw new Error("key file must hold at least 32 bytes");
  return key;
}

// mysql --batch escapes: \\ \t \n \r \0 ; NULL is the bare word NULL.
const UNESCAPE = { "\\": "\\", t: "\t", n: "\n", r: "\r", 0: "\0" };
export function unescapeField(value) {
  return value.replace(/\\([\\tnr0])/g, (_, ch) => UNESCAPE[ch]);
}
export function escapeField(value) {
  return value.replace(/[\\\t\n\r\0]/g, (ch) => ({ "\\": "\\\\", "\t": "\\t", "\n": "\\n", "\r": "\\r", "\0": "\\0" })[ch]);
}

/** Parses `mysql --batch` output (first line is the header). */
export function parseBatchTsv(text) {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  if (lines.at(-1) === "") lines.pop();
  if (lines.length === 0) throw new Error("empty export");
  const header = lines[0].split("\t");
  const rows = lines.slice(1).map((line, index) => {
    const raw = line.split("\t");
    if (raw.length !== header.length) throw new Error(`row ${index + 2}: expected ${header.length} fields, found ${raw.length}`);
    return raw.map((field) => (field === "NULL" ? null : unescapeField(field)));
  });
  return { header, rows };
}

export function formatBatchTsv(header, rows) {
  const body = rows.map((row) => row.map((field) => (field === null ? "NULL" : escapeField(String(field)))).join("\t"));
  return [header.join("\t"), ...body].join("\n") + "\n";
}

export function writeReceipt(file, receipt) {
  if (!file) return;
  mkdirSync(path.dirname(path.resolve(file)), { recursive: true });
  writeFileSync(file, JSON.stringify(receipt, null, 2) + "\n");
}

/** Minimal `--name value` / `--flag` parser; unknown flags are errors. */
export function parseArgs(argv, spec) {
  const out = {};
  for (let i = 0; i < argv.length; i += 1) {
    const name = argv[i].replace(/^--/, "");
    if (!(name in spec)) throw new Error(`unknown option --${name}`);
    if (spec[name] === "flag") out[name] = true;
    else {
      if (i + 1 >= argv.length) throw new Error(`--${name} needs a value`);
      out[name] = argv[++i];
    }
  }
  return out;
}

const IPV4 = /\b(?:25[0-5]|2[0-4]\d|1?\d?\d)(?:\.(?:25[0-5]|2[0-4]\d|1?\d?\d)){3}\b/g;
const EMAIL = /\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b/g;
export const IDENTITY_PATTERNS = { ipv4: IPV4, email: EMAIL };
// Only applied to free text (statements, Profile text values), never to metric
// columns, where an 11- or 18-digit byte count is ordinary.
export const TEXT_PATTERNS = {
  phone_like: /(?<!\d)1[3-9]\d{9}(?!\d)/g,
  id_card_like: /(?<!\d)\d{17}[\dXx](?!\d)/g,
};

/** Replaces IPv4 addresses and e-mail addresses with keyed pseudonyms. */
export function pseudonymizeIdentities(text, pseudonymFn) {
  return text.replace(IPV4, (ip) => pseudonymFn("ip", ip)).replace(EMAIL, (mail) => pseudonymFn("mail", mail));
}
