// Captures the v1.1 visual harness boards (real App + Demo fixtures) at every
// supported breakpoint, for comparison with the approved design boards in
// docs/v1.1/design/boards/ and with an earlier capture.
//
// Start the frontend dev server first (npm --prefix frontend run dev), then:
//   node tests/visual/capture-boards.mjs --out <dir> [--base http://127.0.0.1:5173]
//          [--boards Connect,Main,Overview,Doctor,Compare] [--compare <earlier dir>]
// Writes <Board>-<width>x<height>.png. With --compare, prints the share of pixels
// that differ from the same file in the earlier capture and the box they span.
// Exit code 1 when a board fails to render or scrolls sideways.
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { connectCdp, launchChrome } from "./cdp.mjs";

const option = (name, fallback) => { const i = process.argv.indexOf(name); return i >= 0 ? process.argv[i + 1] : fallback; };
const out = resolve(option("--out", join(tmpdir(), `boards-${Date.now()}`)));
const base = option("--base", "http://127.0.0.1:5173");
const boards = option("--boards", "Connect,Main,Overview,Doctor,Compare").split(",");
const earlier = option("--compare", null);
const sizes = [[1920, 1080], [1440, 900], [1280, 800], [1024, 768], [768, 1024]];
const wait = (ms) => new Promise((done) => setTimeout(done, ms));

mkdirSync(out, { recursive: true });
const { child, debuggerBase } = await launchChrome(mkdtempSync(join(tmpdir(), "srd-boards-")));
const cdp = await connectCdp(debuggerBase);
await cdp.send("Page.enable");
await cdp.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "light" }] });
let failed = false;

/** Share of differing pixels (any channel off by more than 24) and their bounding box, measured in the page. */
async function diff(a, b) {
  const expression = `(async () => {
    const load = (src) => new Promise((ok, no) => { const i = new Image(); i.onload = () => ok(i); i.onerror = no; i.src = src; });
    const [x, y] = await Promise.all([load(${JSON.stringify(a)}), load(${JSON.stringify(b)})]);
    if (x.width !== y.width || x.height !== y.height) return { size: [x.width, x.height, y.width, y.height] };
    const read = (img) => { const c = new OffscreenCanvas(img.width, img.height); const g = c.getContext("2d"); g.drawImage(img, 0, 0); return g.getImageData(0, 0, img.width, img.height).data; };
    const p = read(x), q = read(y); let n = 0, x0 = 1e9, y0 = 1e9, x1 = -1, y1 = -1;
    for (let i = 0; i < p.length; i += 4) {
      if (Math.abs(p[i] - q[i]) > 24 || Math.abs(p[i + 1] - q[i + 1]) > 24 || Math.abs(p[i + 2] - q[i + 2]) > 24) {
        n++; const k = i / 4, px = k % x.width, py = (k - px) / x.width;
        x0 = Math.min(x0, px); y0 = Math.min(y0, py); x1 = Math.max(x1, px); y1 = Math.max(y1, py);
      }
    }
    return { ratio: n / (x.width * x.height), box: n ? [x0, y0, x1, y1] : null };
  })()`;
  const { result } = await cdp.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  return result.value;
}

try {
  for (const [width, height] of sizes) for (const board of boards) {
    await cdp.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
    await cdp.send("Page.navigate", { url: `${base}/visual-harness.html?board=${board}` });
    const deadline = Date.now() + 30000; let error = null;
    for (;;) {
      const { result } = await cdp.send("Runtime.evaluate", { expression: "window.__visualError ? 'ERR:' + window.__visualError : window.__visualReady === true", returnByValue: true });
      if (result.value === true) break;
      if (typeof result.value === "string") { error = result.value; break; }
      if (Date.now() > deadline) { error = "timeout"; break; }
      await wait(100);
    }
    await wait(800);
    const { result: metrics } = await cdp.send("Runtime.evaluate", { expression: "({ scrollWidth: document.documentElement.scrollWidth, width: innerWidth, scrollHeight: document.documentElement.scrollHeight, height: innerHeight })", returnByValue: true });
    const sideways = metrics.value.scrollWidth > metrics.value.width;
    const onePage = metrics.value.scrollHeight <= metrics.value.height;
    const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
    const name = `${board}-${width}x${height}.png`;
    writeFileSync(join(out, name), Buffer.from(data, "base64"));
    let compared = "";
    const previous = earlier ? join(resolve(earlier), name) : null;
    if (previous && existsSync(previous)) {
      const result = await diff(`data:image/png;base64,${readFileSync(previous).toString("base64")}`, `data:image/png;base64,${data}`);
      compared = result.size ? ` size changed ${JSON.stringify(result.size)}` : ` diff ${(result.ratio * 100).toFixed(2)}%${result.box ? ` box ${result.box.join(",")}` : ""}`;
    }
    if (error || sideways) failed = true;
    console.log(`${name} ${error ?? "ok"}${sideways ? " SIDEWAYS-SCROLL" : ""}${width >= 1280 && !onePage ? " page-scroll" : ""}${compared}`);
  }
} finally {
  await cdp.close();
  child.kill();
}
process.exit(failed ? 1 : 0);
