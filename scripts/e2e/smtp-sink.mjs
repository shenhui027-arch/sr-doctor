// Test-only SMTP receiver for the stage D E2E: listens on 127.0.0.1, accepts
// every message (AUTH PLAIN/LOGIN always succeed) and appends one JSON line
// per message to the output file: envelope, decoded subject and text body.
// It never relays anything. Usage: node smtp-sink.mjs <port> <out.jsonl>
import { appendFileSync } from "node:fs";
import { createServer } from "node:net";

const [port, out] = [Number(process.argv[2]), process.argv[3]];
if (!port || !out) {
  console.error("usage: node smtp-sink.mjs <port> <out.jsonl>");
  process.exit(2);
}

const decodeWords = (value) => value.replace(/=\?UTF-8\?B\?([^?]*)\?=/gi, (_, b64) => Buffer.from(b64, "base64").toString("utf8")).replace(/\r\n /g, "");

function parse(raw) {
  const [head, ...rest] = raw.split("\r\n\r\n");
  const headers = head.replace(/\r\n[ \t]+/g, " ");
  const subject = decodeWords(/^Subject: (.*)$/m.exec(headers)?.[1] ?? "");
  const body = rest.join("\r\n\r\n");
  const plain = /Content-Type: text\/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n([A-Za-z0-9+/=\r\n]+?)\r\n--/.exec(body);
  return { subject, text: plain ? Buffer.from(plain[1].replace(/\r\n/g, ""), "base64").toString("utf8") : "" };
}

createServer((socket) => {
  let buffer = "";
  let data = null;
  const envelope = { from: null, to: [], auth: false };
  const reply = (line) => socket.write(`${line}\r\n`);
  reply("220 sr-doctor-e2e-sink");
  socket.on("data", (chunk) => {
    buffer += chunk.toString("latin1");
    let end;
    while ((end = buffer.indexOf("\r\n")) >= 0) {
      const line = buffer.slice(0, end);
      buffer = buffer.slice(end + 2);
      if (data !== null) {
        if (line === ".") {
          const message = parse(Buffer.from(data.join("\r\n"), "latin1").toString("utf8"));
          appendFileSync(out, `${JSON.stringify({ ...envelope, ...message, at: new Date().toISOString() })}\n`);
          data = null;
          envelope.to = [];
          reply("250 queued");
        } else {
          data.push(line.startsWith("..") ? line.slice(1) : line);
        }
        continue;
      }
      const verb = line.split(" ")[0].toUpperCase();
      if (verb === "EHLO" || verb === "HELO") socket.write("250-sink\r\n250 AUTH PLAIN LOGIN\r\n");
      else if (verb === "AUTH") { envelope.auth = true; reply(line.toUpperCase().startsWith("AUTH LOGIN") ? "334 VXNlcm5hbWU6" : "235 ok"); }
      else if (/^[A-Za-z0-9+/=]+$/.test(line) && envelope.auth && !envelope.from) reply("235 ok");
      else if (verb === "MAIL") { envelope.from = /<([^>]*)>/.exec(line)?.[1] ?? null; reply("250 ok"); }
      else if (verb === "RCPT") { envelope.to.push(/<([^>]*)>/.exec(line)?.[1] ?? ""); reply("250 ok"); }
      else if (verb === "DATA") { data = []; reply("354 go"); }
      else if (verb === "QUIT") { reply("221 bye"); socket.end(); }
      else reply("250 ok");
    }
  });
  socket.on("error", () => undefined);
}).listen(port, "127.0.0.1", () => console.log(`listening ${port}`));
