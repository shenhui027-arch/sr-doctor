// Render the pinned Claude Design artboards (`*.dc.html`) to static HTML for
// pixel comparison (v0.7). It expands the design runtime's template subset —
// `<sc-for list="{{…}}" as="x">`, `<sc-if value="{{…}}">` and `{{expr}}` — with
// the data each board's `renderVals()` returns, and swaps the Google Fonts
// link for the product's own bundled font files, so glyphs are identical on
// both sides of the comparison.
//
// Usage: node tests/visual/render-design.mjs --out <dir> [--fonts <dir>]
import { mkdirSync, readFileSync, readdirSync, writeFileSync, copyFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { join, resolve } from "node:path";

const here = import.meta.dirname;
const designDir = join(here, "design-2026-10-20");
const outDir = resolve(option("--out") ?? join(here, "out", "design"));
const repo = resolve(here, "../..");
const fontRoot = join(repo, "frontend", "node_modules", "@fontsource-variable");

export const MANIFEST = JSON.parse(readFileSync(join(designDir, "manifest.json"), "utf8"));

function option(name) {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : undefined;
}

/** Evaluate `{{expr}}` against a scope; the design files are pinned by sha256. */
function evaluate(expr, scope) {
  // eslint-disable-next-line no-new-func
  return Function(...Object.keys(scope), `return (${expr});`)(...Object.values(scope));
}

function interpolate(text, scope) {
  return text.replace(/\{\{([\s\S]*?)\}\}/g, (_, expr) => {
    const value = evaluate(expr.trim(), scope);
    return value === undefined || value === null ? "" : String(value);
  });
}

function expandIfs(text, scope) {
  return text.replace(/<sc-if value="\{\{([\s\S]*?)\}\}"[^>]*>([\s\S]*?)<\/sc-if>/g, (_, expr, body) =>
    evaluate(expr.trim(), scope) ? body : "");
}

function expandFors(text, scope) {
  return text.replace(/<sc-for list="\{\{([\s\S]*?)\}\}" as="(\w+)"[^>]*>([\s\S]*?)<\/sc-for>/g, (_, listExpr, name, body) =>
    evaluate(listExpr.trim(), scope).map((item) => interpolate(expandIfs(body, { ...scope, [name]: item }), { ...scope, [name]: item })).join(""));
}

export function renderBoard(source) {
  const script = /<script type="text\/x-dc"[^>]*>([\s\S]*?)<\/script>/.exec(source);
  let data = {};
  if (script) {
    // eslint-disable-next-line no-new-func
    const Component = Function("DCLogic", `${script[1]}\nreturn Component;`)(class DCLogic {});
    data = new Component().renderVals();
  }
  const helmet = /<helmet>([\s\S]*?)<\/helmet>/.exec(source)?.[1] ?? "";
  const body = /<x-dc>[\s\S]*?<\/helmet>([\s\S]*?)<\/x-dc>/.exec(source)?.[1] ?? "";
  const expanded = interpolate(expandIfs(expandFors(body, data), data), data);
  const head = helmet
    .replace(/<link[^>]*fonts\.(googleapis|gstatic)\.com[^>]*>/g, "")
    .concat(`\n<link rel="stylesheet" href="./fonts.css">`);
  const title = /<title>([\s\S]*?)<\/title>/.exec(source)?.[1] ?? "";
  return `<!doctype html>\n<html lang="zh-CN">\n<head>\n<meta charset="utf-8">\n<title>${title}</title>\n${head}\n</head>\n<body>${expanded}</body>\n</html>\n`;
}

/** The design names Inter / JetBrains Mono / Noto Sans SC; serve them from the product's own variable font files. */
function writeFonts(dir) {
  // The same package CSS the product imports (styles.css), with the family
  // renamed to the name the design asks for and every unicode-range subset kept.
  const packages = [
    ["inter", "index.css", "Inter Variable", "Inter"],
    ["jetbrains-mono", "index.css", "JetBrains Mono Variable", "JetBrains Mono"],
    ["noto-sans-sc", "wght.css", "Noto Sans SC Variable", "Noto Sans SC"],
  ];
  let css = "";
  for (const [pkg, sheet, variableName, designName] of packages) {
    mkdirSync(join(dir, pkg), { recursive: true });
    for (const file of readdirSync(join(fontRoot, pkg, "files")).filter((f) => f.endsWith(".woff2"))) {
      copyFileSync(join(fontRoot, pkg, "files", file), join(dir, pkg, file));
    }
    css += readFileSync(join(fontRoot, pkg, sheet), "utf8")
      .replaceAll(`'${variableName}'`, `'${designName}'`)
      .replaceAll("font-display: swap", "font-display: block")
      .replaceAll("./files/", `./${pkg}/`);
  }
  writeFileSync(join(dir, "fonts.css"), css);
}

if (process.argv[1] && resolve(process.argv[1]) === resolve(import.meta.filename)) {
  mkdirSync(outDir, { recursive: true });
  writeFonts(outDir);
  for (const board of MANIFEST.boards) {
    const source = readFileSync(join(designDir, board.file), "utf8");
    const sha = createHash("sha256").update(source).digest("hex");
    if (sha !== board.sha256) throw new Error(`${board.file} does not match its pinned sha256`);
    writeFileSync(join(outDir, board.file.replace(".dc.html", ".html")), renderBoard(source));
  }
  console.log(JSON.stringify({ status: "PASS", out: outDir, boards: MANIFEST.boards.map((b) => b.file) }));
}
