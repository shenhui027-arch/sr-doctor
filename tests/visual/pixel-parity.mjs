// Pixel parity between the pinned Claude Design boards and the product (v0.7).
//
// For each board: render the design to static HTML (render-design.mjs, the
// product's own font files), capture it and the product harness
// (frontend/visual-harness.html?board=<name>, real App + Demo fixtures) in
// headless Chrome at the board's exact viewport, then compare pixel by pixel
// inside the browser. A pixel matches when every channel differs by at most
// --tolerance (default 24 of 255, enough for antialiasing only).
//
// Usage: node tests/visual/pixel-parity.mjs [--boards Connect,Doctor] [--out dir]
//          [--min-similarity 0.99] [--min-foreground 0.97] [--tolerance 24]
// Exit code 1 when any measured board is below --min-similarity.
import { mkdirSync, writeFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { spawn } from "node:child_process";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { MANIFEST } from "./render-design.mjs";
import { startStaticServer } from "./static-server.mjs";
import { connectCdp, launchChrome, waitForHttp } from "./cdp.mjs";

const here = import.meta.dirname;
const repo = resolve(here, "../..");
const option = (name, fallback) => { const i = process.argv.indexOf(name); return i >= 0 ? process.argv[i + 1] : fallback; };
const outDir = resolve(option("--out", join(tmpdir(), `pixel-parity-${Date.now()}`)));
const minSimilarity = Number(option("--min-similarity", "0.99"));
const minForeground = Number(option("--min-foreground", "0.97"));
const tolerance = Number(option("--tolerance", "24"));
const wanted = option("--boards", "Connect,Main,Doctor,Recommendations,Compare").split(",");
const designPort = 18190 + Math.floor(Math.random() * 400);
const vitePort = designPort + 1;
const wait = (ms) => new Promise((done) => setTimeout(done, ms));

mkdirSync(outDir, { recursive: true });
const designOut = join(outDir, "design-html");
await new Promise((done, reject) => {
  const child = spawn(process.execPath, [join(here, "render-design.mjs"), "--out", designOut], { stdio: "inherit" });
  child.on("exit", (code) => (code === 0 ? done() : reject(new Error("render-design failed"))));
});
const designServer = await startStaticServer(designOut, designPort);
const vite = spawn(process.execPath, [join(repo, "frontend", "node_modules", "vite", "bin", "vite.js"), "--port", String(vitePort), "--strictPort"], { cwd: join(repo, "frontend"), stdio: "ignore", windowsHide: true });
const receipt = { schema_version: "sr-doctor.pixel-parity/v1", design: MANIFEST.source, design_version: MANIFEST.artifact_version, tolerance, min_similarity: minSimilarity, min_foreground_similarity: minForeground, boards: [] };
let chrome; let cdp; let profile;
try {
  await waitForHttp(`http://127.0.0.1:${vitePort}/visual-harness.html`);
  profile = await mkdtemp(join(tmpdir(), "parity-chrome-"));
  chrome = await launchChrome(profile);
  cdp = await connectCdp(chrome.debuggerBase);
  await cdp.send("Page.enable");
  await cdp.send("Runtime.enable");
  await cdp.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "light" }] });
  for (const entry of MANIFEST.boards) {
    const name = entry.file.replace(".dc.html", "");
    if (!wanted.includes(name)) continue;
    await cdp.send("Emulation.setDeviceMetricsOverride", { width: entry.width, height: entry.height, deviceScaleFactor: 1, mobile: false });
    const design = await capture(`http://127.0.0.1:${designPort}/${name}.html`, entry, "document.fonts.ready.then(() => true)");
    let product; let error = null;
    try {
      product = await capture(`http://127.0.0.1:${vitePort}/visual-harness.html?board=${name}`, entry, "window.__visualError ? Promise.reject(new Error(window.__visualError)) : window.__visualReady === true");
    } catch (failure) { error = failure instanceof Error ? failure.message : String(failure); }
    const files = { design: join(outDir, `${name}.design.png`), product: product ? join(outDir, `${name}.product.png`) : null, diff: product ? join(outDir, `${name}.diff.png`) : null };
    writeFileSync(files.design, Buffer.from(design, "base64"));
    let comparison = null;
    if (product) {
      writeFileSync(files.product, Buffer.from(product, "base64"));
      comparison = await compare(design, product, entry);
      writeFileSync(files.diff, Buffer.from(comparison.diff, "base64"));
    }
    const result = { board: name, title: entry.title, width: entry.width, height: entry.height,
      similarity: comparison ? comparison.similarity : null, foreground_similarity: comparison ? comparison.foreground : null, mismatched_pixels: comparison ? comparison.mismatched : null,
      rows_with_mismatch: comparison ? comparison.rows : null, error, files };
    receipt.boards.push(result);
    console.log(`${name}: ${comparison ? "page " + (comparison.similarity * 100).toFixed(2) + "% · foreground " + (comparison.foreground * 100).toFixed(2) + "%" : "ERROR " + error}`);
  }
} finally {
  await cdp?.close();
  chrome?.child.kill();
  vite.kill();
  designServer.close();
  if (profile) await rm(profile, { recursive: true, force: true }).catch(() => undefined);
}
receipt.status = receipt.boards.every((b) => b.similarity !== null && b.similarity >= minSimilarity && b.foreground_similarity >= minForeground) ? "PASS" : "FAIL";
writeFileSync(join(outDir, "pixel-parity-receipt.json"), `${JSON.stringify(receipt, null, 2)}\n`);
console.log(JSON.stringify({ status: receipt.status, out: outDir }));
if (receipt.status !== "PASS") process.exitCode = 1;

async function capture(url, entry, readyExpression) {
  await cdp.send("Page.navigate", { url });
  const deadline = Date.now() + 30_000;
  for (;;) {
    const { result, exceptionDetails } = await cdp.send("Runtime.evaluate", { expression: readyExpression, awaitPromise: true, returnByValue: true });
    if (exceptionDetails) throw new Error(exceptionDetails.exception?.description ?? exceptionDetails.text);
    if (result.value === true) break;
    if (Date.now() > deadline) throw new Error(`${url} was not ready in time`);
    await wait(100);
  }
  await wait(300);
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png", clip: { x: 0, y: 0, width: entry.width, height: entry.height, scale: 1 }, captureBeyondViewport: false });
  return data;
}

async function compare(designPng, productPng, entry) {
  await cdp.send("Page.navigate", { url: "about:blank" });
  await wait(100);
  const expression = `(async () => {
    const load = (b64) => new Promise((done, fail) => { const i = new Image(); i.onload = () => done(i); i.onerror = fail; i.src = "data:image/png;base64," + b64; });
    const [a, b] = await Promise.all([load(${JSON.stringify(designPng)}), load(${JSON.stringify(productPng)})]);
    const w = ${entry.width}, h = ${entry.height};
    const canvas = (img) => { const c = document.createElement("canvas"); c.width = w; c.height = h; const x = c.getContext("2d"); x.fillStyle = "#fff"; x.fillRect(0, 0, w, h); x.drawImage(img, 0, 0); return x.getImageData(0, 0, w, h); };
    const da = canvas(a).data, db = canvas(b).data;
    const out = document.createElement("canvas"); out.width = w; out.height = h; const ox = out.getContext("2d"); const diff = ox.createImageData(w, h);
    // Foreground: pixels that are not the page background (#F5F6F8) in either image.
    const isPage = (d, i) => Math.abs(d[i] - 245) <= 3 && Math.abs(d[i + 1] - 246) <= 3 && Math.abs(d[i + 2] - 248) <= 3;
    let mismatched = 0, foreground = 0, foregroundMismatched = 0; const rows = new Set();
    for (let p = 0; p < w * h; p++) {
      const i = p * 4;
      const delta = Math.max(Math.abs(da[i] - db[i]), Math.abs(da[i + 1] - db[i + 1]), Math.abs(da[i + 2] - db[i + 2]));
      const gray = (da[i] + da[i + 1] + da[i + 2]) / 3;
      const fg = !(isPage(da, i) && isPage(db, i));
      if (fg) foreground++;
      if (delta > ${tolerance}) { mismatched++; if (fg) foregroundMismatched++; rows.add(Math.floor(p / w)); diff.data[i] = 230; diff.data[i + 1] = 30; diff.data[i + 2] = 30; diff.data[i + 3] = 255; }
      else { diff.data[i] = diff.data[i + 1] = diff.data[i + 2] = 255 - (255 - gray) * 0.25; diff.data[i + 3] = 255; }
    }
    ox.putImageData(diff, 0, 0);
    return { mismatched, rows: rows.size, similarity: 1 - mismatched / (w * h), foreground: foreground ? 1 - foregroundMismatched / foreground : 1, diff: out.toDataURL("image/png").split(",")[1] };
  })()`;
  const { result, exceptionDetails } = await cdp.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (exceptionDetails) throw new Error(exceptionDetails.exception?.description ?? exceptionDetails.text);
  return result.value;
}
