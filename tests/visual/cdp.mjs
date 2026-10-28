// Headless Chrome over CDP for the visual parity tools (v0.7).
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { spawn } from "node:child_process";
import { join } from "node:path";

const wait = (ms) => new Promise((done) => setTimeout(done, ms));

export async function waitForHttp(url) {
  const deadline = Date.now() + 60_000;
  for (;;) {
    try { if ((await fetch(url)).ok) return; } catch { /* starting */ }
    if (Date.now() > deadline) throw new Error(`${url} did not start`);
    await wait(250);
  }
}

export async function launchChrome(dir) {
  const executable = process.env.CHROME_BIN ?? ["C:/Program Files/Google/Chrome/Application/chrome.exe", "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe"].find(existsSync);
  if (!executable) throw new Error("Chrome or Edge executable was not found. Set CHROME_BIN.");
  const child = spawn(executable, ["--headless=new", "--disable-gpu", "--hide-scrollbars", "--force-color-profile=srgb", "--font-render-hinting=none", "--no-first-run", "--no-default-browser-check", "--remote-debugging-port=0", `--user-data-dir=${dir}`, "about:blank"], { stdio: "ignore", windowsHide: true });
  const activePort = join(dir, "DevToolsActivePort");
  const deadline = Date.now() + 30_000;
  while (!existsSync(activePort)) { if (Date.now() > deadline) throw new Error("Chrome DevTools endpoint did not start"); await wait(100); }
  await wait(100);
  const [port] = (await readFile(activePort, "utf8")).trim().split(/\r?\n/);
  return { child, debuggerBase: `http://127.0.0.1:${port}` };
}

export async function connectCdp(debuggerBase) {
  const targets = await (await fetch(`${debuggerBase}/json/list`)).json();
  const target = targets.find((item) => item.type === "page") ?? await (await fetch(`${debuggerBase}/json/new?about:blank`, { method: "PUT" })).json();
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((done, reject) => { socket.addEventListener("open", done, { once: true }); socket.addEventListener("error", reject, { once: true }); });
  let sequence = 0; const pending = new Map();
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(String(event.data));
    const request = message.id ? pending.get(message.id) : null;
    if (!request) return;
    pending.delete(message.id);
    if (message.error) request.reject(new Error(`CDP ${message.error.code}: ${message.error.message}`)); else request.resolve(message.result ?? {});
  });
  return {
    send(method, params = {}) { const id = ++sequence; socket.send(JSON.stringify({ id, method, params })); return new Promise((done, reject) => pending.set(id, { resolve: done, reject })); },
    close: async () => socket.close(),
  };
}
