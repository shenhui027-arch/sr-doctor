import test from "node:test";
import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";

// User-facing documentation must not link to files or headings that do not
// exist. Version records and task receipts are historical and not checked.
const root = resolve(".");
const pages = [
  "README.md", "README.en.md", "README.zh-TW.md", "CHANGELOG.md", "CONTRIBUTING.md", "SECURITY.md",
  "docs/README.md", "CODE_OF_CONDUCT.md",
  ...["docs/guide", "docs/development"].flatMap((dir) => readdirSync(dir).filter((name) => name.endsWith(".md")).map((name) => `${dir}/${name}`)),
  ...readdirSync("docs").filter((name) => /^\d\d-.+\.md$/.test(name)).map((name) => `docs/${name}`),
  // Version records exist only in the private repository.
  ...["docs/v1.0/README.md", "docs/v0.1/00-initial-design-index.md"].filter((page) => existsSync(page)),
];

/** GitHub's heading anchors: lower case, punctuation dropped, spaces to hyphens, repeats numbered. */
function anchors(markdown) {
  const seen = new Map();
  const result = new Set();
  let fenced = false;
  for (const line of markdown.split(/\r?\n/)) {
    if (/^\s*```/.test(line)) fenced = !fenced;
    const heading = !fenced && /^#{1,6}\s+(.+?)\s*#*\s*$/.exec(line);
    if (!heading) continue;
    const text = heading[1].replace(/`/g, "").replace(/\[([^\]]*)\]\([^)]*\)/g, "$1");
    const base = text.toLowerCase().replace(/[^\p{L}\p{M}\p{N}\p{Pc}\- ]/gu, "").replace(/ /g, "-");
    const count = seen.get(base) ?? 0;
    seen.set(base, count + 1);
    result.add(count ? `${base}-${count}` : base);
  }
  return result;
}

function links(markdown) {
  const withoutCode = markdown.replace(/```[\s\S]*?```/g, "").replace(/`[^`\n]*`/g, "");
  const found = [];
  for (const match of withoutCode.matchAll(/\]\(([^)\s]+)(?:\s+"[^"]*")?\)/g)) found.push(match[1]);
  for (const match of withoutCode.matchAll(/<(?:img|a)\b[^>]*\b(?:src|href)="([^"]+)"/g)) found.push(match[1]);
  return found;
}

for (const page of pages) {
  test(`links in ${page} resolve`, () => {
    const markdown = readFileSync(page, "utf8");
    const broken = [];
    for (const target of links(markdown)) {
      if (/^(?:https?:|mailto:)/.test(target)) continue;
      const [path, hash] = target.split("#");
      const file = path ? resolve(dirname(page), decodeURI(path)) : resolve(page);
      if (!file.startsWith(root) || !existsSync(file)) { broken.push(`${target} (missing file)`); continue; }
      if (hash && file.endsWith(".md") && statSync(file).isFile() && !anchors(readFileSync(file, "utf8")).has(decodeURI(hash))) {
        broken.push(`${target} (missing heading in ${relative(root, file)})`);
      }
    }
    assert.deepEqual(broken, []);
  });
}

test("every guide page is reachable from the documentation hub", () => {
  const hub = readFileSync("docs/README.md", "utf8");
  for (const name of [...readdirSync("docs/guide"), ...readdirSync("docs/development")]) {
    const dir = existsSync(join("docs/guide", name)) ? "guide" : "development";
    assert.ok(hub.includes(`](${dir}/${name})`), `${dir}/${name} is not linked from docs/README.md`);
  }
});
