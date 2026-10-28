// One-shot downgrade relay for the v0.7-01 TLS E2E (test infrastructure only).
//
// It accepts exactly one loopback client, forwards the real StarRocks
// HandshakeV10 greeting with the CLIENT_SSL (0x0800) capability bit cleared,
// and then only counts what the client sends. Nothing the client sends is
// forwarded, stored or printed: the receipt holds byte counts and booleans.
import { createServer, connect } from "node:net";
import { writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

export const CLIENT_SSL = 0x0800;

// Returns the capability-lower offset inside a complete greeting packet
// (4-byte header + HandshakeV10 payload), or throws on any other shape.
export function capabilityOffset(packet) {
  if (packet.length < 4) throw new Error("short packet");
  const length = packet[0] | (packet[1] << 8) | (packet[2] << 16);
  if (packet.length !== 4 + length || packet[3] !== 0) throw new Error("not a complete first packet");
  if (packet[4] !== 10) throw new Error("not HandshakeV10");
  const versionEnd = packet.indexOf(0, 5);
  if (versionEnd < 0) throw new Error("unterminated server version");
  // connection id (4) + auth-plugin-data-part-1 (8) + filler (1)
  const offset = versionEnd + 1 + 4 + 8 + 1;
  if (offset + 2 > packet.length) throw new Error("truncated capabilities");
  return offset;
}

export function stripSslCapability(packet) {
  const offset = capabilityOffset(packet);
  const copy = Buffer.from(packet);
  const before = copy.readUInt16LE(offset);
  copy.writeUInt16LE(before & ~CLIENT_SSL, offset);
  return { packet: copy, sslAdvertised: (before & CLIENT_SSL) !== 0 };
}

function run(listenPort, upstreamPort, reportPath, timeoutMs) {
  const report = { status: "NOT_RUN", upstream_ssl_advertised: null, greeting_forwarded_without_ssl: false, client_bytes_sent: 0, client_closed: false };
  let finished = false;
  const server = createServer({ pauseOnConnect: false });
  const finish = (status) => {
    if (finished) return;
    finished = true;
    report.status = status;
    writeFileSync(reportPath, JSON.stringify(report) + "\n", { flag: "wx" });
    server.close();
    setTimeout(() => process.exit(0), 50).unref();
  };
  const timer = setTimeout(() => finish("timeout"), timeoutMs);
  server.on("error", () => finish("listen_failed"));
  server.once("connection", (client) => {
    server.close();
    const upstream = connect({ host: "127.0.0.1", port: upstreamPort });
    let pending = Buffer.alloc(0);
    let greeted = false;
    const close = () => { clearTimeout(timer); client.destroy(); upstream.destroy(); finish(greeted ? "completed" : "upstream_failed"); };
    upstream.on("data", (chunk) => {
      if (greeted) return;
      pending = Buffer.concat([pending, chunk]);
      if (pending.length < 4) return;
      const total = 4 + (pending[0] | (pending[1] << 8) | (pending[2] << 16));
      if (pending.length < total) return;
      try {
        const { packet, sslAdvertised } = stripSslCapability(pending.subarray(0, total));
        report.upstream_ssl_advertised = sslAdvertised;
        greeted = true;
        client.write(packet, () => { report.greeting_forwarded_without_ssl = true; });
      } catch {
        close();
      }
    });
    client.on("data", (chunk) => { report.client_bytes_sent += chunk.length; });
    client.on("close", () => { report.client_closed = true; close(); });
    client.on("error", close);
    upstream.on("error", close);
  });
  // A marker file, not a probe connection, signals readiness: the relay
  // accepts exactly one client.
  server.listen(listenPort, "127.0.0.1", () => writeFileSync(`${reportPath}.listening`, "", { flag: "wx" }));
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [listenPort, upstreamPort, reportPath] = process.argv.slice(2);
  run(Number(listenPort), Number(upstreamPort), reportPath, 60_000);
}
