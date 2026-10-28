// Minimal loopback static file server for the visual parity harness (v0.7).
// Usage: node tests/visual/static-server.mjs --root <dir> --port <port>
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize, resolve } from "node:path";

const TYPES = { ".html": "text/html; charset=utf-8", ".css": "text/css", ".js": "text/javascript", ".woff2": "font/woff2", ".png": "image/png", ".json": "application/json" };

export function startStaticServer(root, port) {
  const base = resolve(root);
  const server = createServer(async (request, response) => {
    const path = normalize(join(base, decodeURIComponent(new URL(request.url, "http://x").pathname)));
    if (!path.startsWith(base)) { response.writeHead(403).end(); return; }
    try {
      const body = await readFile(path);
      response.writeHead(200, { "Content-Type": TYPES[extname(path)] ?? "application/octet-stream" }).end(body);
    } catch {
      response.writeHead(404).end();
    }
  });
  return new Promise((done) => server.listen(port, "127.0.0.1", () => done(server)));
}

if (process.argv[1] && resolve(process.argv[1]) === resolve(import.meta.filename)) {
  const arg = (name) => process.argv[process.argv.indexOf(name) + 1];
  await startStaticServer(arg("--root"), Number(arg("--port")));
  console.log(`serving ${arg("--root")} on http://127.0.0.1:${arg("--port")}`);
}
