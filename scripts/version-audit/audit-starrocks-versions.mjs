#!/usr/bin/env node
// Multi-version source audit: fingerprints the official StarRocks source this
// product depends on (scripts/version-audit/watchlist.json) at every release
// tag of the supported minor lines, and lists the commits that touched it.
//
//   node scripts/version-audit/audit-starrocks-versions.mjs --repo <starrocks.git> --out <dir>
//
// <starrocks.git> is any clone of github.com/StarRocks/starrocks with tags; a
// blobless clone (`git clone --bare --filter=blob:none`) is enough. The audit
// never writes to it except to fetch missing blobs. Output: version-audit.json
// (machine readable) and version-audit.md (review notes).
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../..");

export function parseArgs(argv) {
  const args = { repo: null, out: null, watchlist: join(here, "watchlist.json"), pack: join(repoRoot, "backend/crates/version-packs/starrocks_3_5_14/version-pack.json"), commits: true };
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    const value = () => {
      const next = argv[index + 1];
      if (next === undefined || next.startsWith("--")) throw new Error(`${flag} needs a value`);
      index += 1;
      return next;
    };
    if (flag === "--repo") args.repo = value();
    else if (flag === "--out") args.out = value();
    else if (flag === "--watchlist") args.watchlist = value();
    else if (flag === "--pack") args.pack = value();
    else if (flag === "--no-commits") args.commits = false;
    else throw new Error(`unknown option ${flag}`);
  }
  if (!args.repo || !args.out) throw new Error("usage: --repo <starrocks.git> --out <dir> [--watchlist file] [--pack version-pack.json] [--no-commits]");
  return args;
}

/** Release tags (x.y.z) of the given minor lines, in semantic order. */
export function releaseTags(allTags, lines) {
  const wanted = new Set(lines);
  return allTags
    .map((tag) => tag.trim())
    .filter((tag) => /^\d+\.\d+\.\d+$/.test(tag) && wanted.has(tag.split(".").slice(0, 2).join(".")))
    .sort(compareVersions);
}

export function compareVersions(left, right) {
  const a = left.split(".").map(Number);
  const b = right.split(".").map(Number);
  for (let index = 0; index < 3; index += 1) if (a[index] !== b[index]) return a[index] - b[index];
  return 0;
}

/** A Java method or C++ function body found by its signature text, braces balanced. */
export function extractBlock(source, match, { cpp = false } = {}) {
  const lines = source.split(/\r?\n/);
  for (let start = 0; start < lines.length; start += 1) {
    const line = lines[start];
    if (!line.includes(match)) continue;
    // C++: a definition, not a call or declaration. Java: a method header.
    if (cpp ? /;\s*$/.test(line) || /^\s*(return|auto|if)\b/.test(line) : !/\b(public|private|protected|static)\b/.test(line)) continue;
    let depth = 0;
    let opened = false;
    const body = [];
    for (let index = start; index < lines.length; index += 1) {
      const text = lines[index];
      body.push(text);
      const code = text.replace(/"(?:\\.|[^"\\])*"/g, "\"\"").replace(/\/\/.*$/, "");
      for (const char of code) {
        if (char === "{") { depth += 1; opened = true; }
        else if (char === "}") depth -= 1;
      }
      if (!opened && /;\s*$/.test(code)) break;
      if (opened && depth <= 0) return body.join("\n");
    }
  }
  return null;
}

/** Thrift enum/struct blocks, in the order listed. */
export function extractThriftBlocks(source, heads) {
  const parts = [];
  for (const head of heads) {
    const at = source.indexOf(head);
    if (at < 0) { parts.push(`${head}: absent`); continue; }
    const end = source.indexOf("}", at);
    parts.push(source.slice(at, end + 1));
  }
  return parts.join("\n");
}

/** Whitespace and comments do not change semantics; strip them before hashing. */
export function fingerprint(text) {
  if (text === null) return null;
  const normalized = text.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/.*$/gm, "").replace(/\s+/g, " ").trim();
  return createHash("sha256").update(normalized).digest("hex").slice(0, 12);
}

/** Consecutive tags with the same value collapse into one segment. */
export function segments(tags, valueOf) {
  const result = [];
  for (const tag of tags) {
    const value = valueOf(tag);
    const last = result.at(-1);
    if (last && last.value === value) last.to = tag;
    else result.push({ from: tag, to: tag, value });
  }
  return result;
}

class Repo {
  constructor(path) {
    this.path = path;
    this.blobCache = new Map();
  }

  git(args, options = {}) {
    return execFileSync("git", ["--git-dir", this.path, ...args], { encoding: "utf8", maxBuffer: 256 * 1024 * 1024, stdio: ["ignore", "pipe", "pipe"], ...options });
  }

  tags() {
    return this.git(["tag", "--list"]).split("\n").filter(Boolean);
  }

  /** Object ids of the given paths at a tag; absent paths are left out. */
  objectIds(tag, paths) {
    const ids = new Map();
    const output = this.git(["ls-tree", "-r", tag, "--", ...paths]);
    for (const line of output.split("\n")) {
      const match = /^\d+ blob ([0-9a-f]+)\t(.+)$/.exec(line);
      if (match) ids.set(match[2], match[1]);
    }
    return ids;
  }

  /** One fetch for many missing blobs, instead of one lazy fetch per blob. */
  prefetch(objectIds) {
    const missing = [...new Set(objectIds)].filter((id) => {
      try { this.git(["cat-file", "-e", id], { env: { ...process.env, GIT_NO_LAZY_FETCH: "1" } }); return false; } catch { return true; }
    });
    for (let index = 0; index < missing.length; index += 200) {
      const chunk = missing.slice(index, index + 200);
      try {
        this.git(["-c", "fetch.negotiationAlgorithm=noop", "fetch", "--no-tags", "--no-write-fetch-head", "--recurse-submodules=no", "--filter=blob:none", "origin", ...chunk]);
      } catch {
        // Lazy fetch on read still works; prefetching only saves round trips.
      }
    }
    return missing.length;
  }

  blob(id) {
    if (!this.blobCache.has(id)) this.blobCache.set(id, this.git(["cat-file", "blob", id]));
    return this.blobCache.get(id);
  }
}

export function audit({ repo, watchlist, registryMetrics, commits }) {
  const tags = releaseTags(repo.tags(), watchlist.lines);
  if (!tags.includes(watchlist.baseline)) throw new Error(`baseline ${watchlist.baseline} is not a tag of the audited lines`);
  const paths = [...new Set([
    ...watchlist.functions.flatMap((entry) => entry.paths),
    ...watchlist.metric_emitters,
    ...watchlist.capabilities.flatMap((entry) => entry.file_exists ?? entry.grep.paths),
    ...watchlist.audit_fields.paths,
  ])];
  const treeByTag = new Map(tags.map((tag) => [tag, repo.objectIds(tag, paths)]));
  const fetched = repo.prefetch([...treeByTag.values()].flatMap((ids) => [...ids.values()]));
  const read = (tag, path) => {
    const id = treeByTag.get(tag).get(path);
    return id ? repo.blob(id) : null;
  };
  const firstExisting = (tag, candidates) => candidates.map((path) => read(tag, path)).find((text) => text !== null) ?? null;

  const functions = watchlist.functions.map((entry) => {
    const valueOf = (tag) => {
      const source = firstExisting(tag, entry.paths);
      if (source === null) return "absent";
      const block = entry.block ? extractThriftBlocks(source, entry.block) : extractBlock(source, entry.cpp ?? entry.match, { cpp: Boolean(entry.cpp) });
      return block === null ? "absent" : fingerprint(block);
    };
    const values = new Map(tags.map((tag) => [tag, valueOf(tag)]));
    return { id: entry.id, area: entry.area, why: entry.why, baseline: values.get(watchlist.baseline), segments: segments(tags, (tag) => values.get(tag)) };
  });

  const emitterText = (tag) => watchlist.metric_emitters.map((path) => read(tag, path) ?? "").join("\n");
  const baselineEmitters = emitterText(watchlist.baseline);
  const literal = registryMetrics.filter((metric) => baselineEmitters.includes(`"${metric}"`));
  const metrics = literal.map((metric) => ({ metric, segments: segments(tags, (tag) => emitterText(tag).includes(`"${metric}"`) ? "present" : "absent") }));

  const capabilities = watchlist.capabilities.map((entry) => ({
    id: entry.id,
    why: entry.why,
    segments: segments(tags, (tag) => {
      if (entry.file_exists) return entry.file_exists.some((path) => treeByTag.get(tag).has(path)) ? "yes" : "no";
      return entry.grep.paths.some((path) => (read(tag, path) ?? "").includes(entry.grep.pattern)) ? "yes" : "no";
    }),
  }));

  const auditPattern = new RegExp(watchlist.audit_fields.pattern, "g");
  const auditFieldsAt = (tag) => [...(firstExisting(tag, watchlist.audit_fields.paths) ?? "").matchAll(auditPattern)].map((match) => match[1]);
  const auditFields = segments(tags, (tag) => auditFieldsAt(tag).join(","))
    .map((segment) => ({ from: segment.from, to: segment.to, fields: segment.value ? segment.value.split(",") : [] }));

  const lineHeads = watchlist.lines.map((line) => tags.filter((tag) => tag.startsWith(`${line}.`)).at(-1)).filter(Boolean);
  const intervals = [];
  if (commits) {
    const boundaries = [...new Set([...lineHeads, watchlist.baseline])].sort(compareVersions);
    for (let index = 1; index < boundaries.length; index += 1) {
      const [from, to] = [boundaries[index - 1], boundaries[index]];
      const log = repo.git(["log", "--no-merges", "--format=%h%x09%ad%x09%s", "--date=short", `${from}..${to}`, "--", ...watchlist.watch_files]);
      const total = Number(repo.git(["rev-list", "--no-merges", "--count", `${from}..${to}`]).trim());
      const list = log.split("\n").filter(Boolean).map((line) => {
        const [hash, date, subject] = line.split("\t");
        return { hash, date, subject };
      });
      intervals.push({ from, to, total_commits: total, watch_commits: list.length, commits: list });
    }
  }

  return { baseline: watchlist.baseline, tags, line_heads: lineHeads, prefetched_blobs: fetched, functions, metrics, unlisted_metrics: registryMetrics.filter((metric) => !literal.includes(metric)), capabilities, audit_fields: auditFields, intervals };
}

export function renderMarkdown(result) {
  const out = [];
  const range = (segment) => segment.from === segment.to ? segment.from : `${segment.from}～${segment.to}`;
  out.push(`# StarRocks 多版本源码审计`, "", `基线：${result.baseline}；审计标签：${result.tags.length} 个（${result.tags[0]}～${result.tags.at(-1)}）。本文件由 \`scripts/version-audit/audit-starrocks-versions.mjs\` 生成。`, "");
  out.push("## 语义函数", "", "同一指纹的连续版本合并为一段；与基线相同标 ✅。", "", "| 函数 | 领域 | 分段 |", "|---|---|---|");
  for (const entry of result.functions) {
    const parts = entry.segments.map((segment) => `${range(segment)} ${segment.value === "absent" ? "不存在" : segment.value === entry.baseline ? "✅" : `\`${segment.value}\``}`);
    out.push(`| \`${entry.id}\` | ${entry.area} | ${parts.join("<br>")} |`);
  }
  out.push("", "## 注册指标的产生位置", "", "| 指标 | 分段 |", "|---|---|");
  for (const entry of result.metrics) {
    if (entry.segments.length === 1 && entry.segments[0].value === "present") continue;
    out.push(`| \`${entry.metric}\` | ${entry.segments.map((segment) => `${range(segment)} ${segment.value === "present" ? "有" : "无"}`).join("<br>")} |`);
  }
  out.push("", `所有审计版本都存在的指标未列出。基线源码中没有字面量、由其他方式产生的指标：${result.unlisted_metrics.map((metric) => `\`${metric}\``).join("、") || "无"}。`);
  out.push("", "## 连接侧能力", "", "| 能力 | 分段 |", "|---|---|");
  for (const entry of result.capabilities) out.push(`| \`${entry.id}\` | ${entry.segments.map((segment) => `${range(segment)} ${segment.value === "yes" ? "有" : "无"}`).join("<br>")} |`);
  out.push("", "## 审计事件字段", "");
  let previous = null;
  for (const segment of result.audit_fields) {
    const added = previous ? segment.fields.filter((field) => !previous.includes(field)) : segment.fields;
    const removed = previous ? previous.filter((field) => !segment.fields.includes(field)) : [];
    out.push(`- **${range(segment)}**：${previous ? `新增 ${added.join("、") || "无"}；删除 ${removed.join("、") || "无"}` : `${segment.fields.length} 个字段`}`);
    previous = segment.fields;
  }
  if (result.intervals.length) {
    out.push("", "## 触及监控文件的提交", "", "| 区间 | 全部提交 | 触及监控文件 |", "|---|---|---|");
    for (const interval of result.intervals) out.push(`| ${interval.from} → ${interval.to} | ${interval.total_commits} | ${interval.watch_commits} |`);
    out.push("", "逐条提交见 `version-audit.json` 的 `intervals[].commits`。");
  }
  return `${out.join("\n")}\n`;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const watchlist = JSON.parse(readFileSync(args.watchlist, "utf8"));
  const pack = JSON.parse(readFileSync(args.pack, "utf8"));
  const registryMetrics = pack.metric_registry.metrics.map((entry) => entry.metric_id);
  const result = audit({ repo: new Repo(resolve(args.repo)), watchlist, registryMetrics, commits: args.commits });
  mkdirSync(args.out, { recursive: true });
  writeFileSync(join(args.out, "version-audit.json"), `${JSON.stringify(result, null, 2)}\n`);
  writeFileSync(join(args.out, "version-audit.md"), renderMarkdown(result));
  process.stdout.write(`audited ${result.tags.length} tags (${result.prefetched_blobs} blobs fetched) -> ${args.out}\n`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
