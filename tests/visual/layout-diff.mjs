// Element-level diff between a design board and the product harness (v0.7).
// Pairs visible text runs by their text and lists position, size, font and
// colour differences, so pixel gaps can be fixed at the box that causes them.
//
// Usage: node tests/visual/layout-diff.mjs --board Connect --design http://127.0.0.1:18190
//          --product http://127.0.0.1:18300 [--ymax 60] [--all]
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { MANIFEST } from "./render-design.mjs";
import { connectCdp, launchChrome } from "./cdp.mjs";

const option = (name, fallback) => { const i = process.argv.indexOf(name); return i >= 0 ? process.argv[i + 1] : fallback; };
const board = option("--board", "Connect");
const designBase = option("--design", "http://127.0.0.1:18190");
const productBase = option("--product", "http://127.0.0.1:18300");
const ymin = Number(option("--ymin", "-1")); const ymax = Number(option("--ymax", "100000"));
const showAll = process.argv.includes("--all");
const entry = MANIFEST.boards.find((b) => b.file === `${board}.dc.html`);
const dumpSource = await readFile(join(import.meta.dirname, "layout-dump.js"), "utf8");
const wait = (ms) => new Promise((done) => setTimeout(done, ms));

const profile = await mkdtemp(join(tmpdir(), "layout-diff-"));
const chrome = await launchChrome(profile);
const cdp = await connectCdp(chrome.debuggerBase);
try {
  await cdp.send("Page.enable"); await cdp.send("Runtime.enable");
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: entry.width, height: entry.height, deviceScaleFactor: 1, mobile: false });
  const design = await dump(`${designBase}/${board}.html`, "document.fonts.ready.then(() => true)");
  const product = await dump(`${productBase}/visual-harness.html?board=${board}`, "window.__visualReady === true");
  const inRange = (item) => item.y >= ymin && item.y <= ymax;
  const texts = (list) => list.filter((i) => i.text && inRange(i));
  const used = new Set();
  const rows = [];
  for (const d of texts(design)) {
    const index = product.findIndex((p, k) => !used.has(k) && p.text === d.text && inRange(p));
    if (index < 0) { rows.push(`MISSING  ${JSON.stringify(d.text)} @${d.x},${d.y} ${d.w}x${d.h} ${d.font}`); continue; }
    used.add(index);
    const p = product[index];
    const delta = [];
    if (Math.abs(p.x - d.x) > 0 || Math.abs(p.y - d.y) > 0) delta.push(`pos ${d.x},${d.y}→${p.x},${p.y}`);
    if (Math.abs(p.w - d.w) > 1 || Math.abs(p.h - d.h) > 1) delta.push(`size ${d.w}x${d.h}→${p.w}x${p.h}`);
    if (p.font !== d.font) delta.push(`font ${d.font}→${p.font}`);
    if (p.color !== d.color) delta.push(`color ${d.color}→${p.color}`);
    if (delta.length || showAll) rows.push(`${delta.length ? "DIFF" : "OK  "}     ${JSON.stringify(d.text)} ${delta.join(" · ")}`);
  }
  product.forEach((p, k) => { if (p.text && inRange(p) && !used.has(k)) rows.push(`EXTRA    ${JSON.stringify(p.text)} @${p.x},${p.y} ${p.w}x${p.h} ${p.font}`); });
  console.log(rows.join("\n"));
} finally {
  await cdp.close(); chrome.child.kill(); await rm(profile, { recursive: true, force: true }).catch(() => undefined);
}

async function dump(url, ready) {
  await cdp.send("Page.navigate", { url });
  const deadline = Date.now() + 30_000;
  for (;;) {
    const { result } = await cdp.send("Runtime.evaluate", { expression: ready, awaitPromise: true, returnByValue: true });
    if (result.value === true) break;
    if (Date.now() > deadline) throw new Error(`${url} not ready`);
    await wait(100);
  }
  await wait(300);
  const { result } = await cdp.send("Runtime.evaluate", { expression: dumpSource, returnByValue: true });
  return result.value;
}
