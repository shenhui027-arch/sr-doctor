// Test infrastructure only: hands the fixture reader password, inherited from
// the E2E runner's process environment, to the product page on 127.0.0.1
// exactly once, so a browser check can sign in without the value appearing in
// any command line, file or transcript. GET /release (or the timeout) ends it.
import { createServer } from "node:http";

const password = process.env.A1_READER_PASSWORD;
const origin = process.env.A1_PRODUCT_ORIGIN;
const port = Number(process.env.A1_PASSWORD_PORT);
const timeoutMs = Number(process.env.A1_HOLD_SECONDS) * 1000;
if (!password || !origin || !Number.isInteger(port) || !(timeoutMs > 0)) process.exit(2);

let served = false;
const server = createServer((request, response) => {
  if (request.url === "/release") {
    response.writeHead(204, { Connection: "close" }).end();
    server.close();
    server.closeAllConnections();
    return;
  }
  if (request.url !== "/password" || request.headers.origin !== origin || served) {
    response.writeHead(403).end();
    return;
  }
  served = true;
  response.writeHead(200, { "Access-Control-Allow-Origin": origin, "Cache-Control": "no-store", "Content-Type": "text/plain" });
  response.end(password);
});
server.listen(port, "127.0.0.1", () => console.log("listening"));
server.on("close", () => {
  console.log(served ? "served_once" : "not_served");
  process.exit(0);
});
setTimeout(() => {
  server.close();
  server.closeAllConnections();
}, timeoutMs).unref();
