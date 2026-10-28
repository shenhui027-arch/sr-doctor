import assert from "node:assert/strict";
import test from "node:test";

import { CLIENT_SSL, capabilityOffset, stripSslCapability } from "./tls-strip-relay.mjs";

// HandshakeV10 greeting: version 10, "8.0.33\0", connection id, 8 nonce
// bytes, filler, capability-lower, then the rest of a real greeting.
function greeting(capabilityLower) {
  const payload = Buffer.concat([
    Buffer.from([10]), Buffer.from("8.0.33\0", "latin1"), Buffer.from([1, 0, 0, 0]),
    Buffer.alloc(8, 0x41), Buffer.from([0]), Buffer.from([capabilityLower & 0xff, capabilityLower >> 8]),
    Buffer.from([33, 2, 0, 0x08, 0x80, 21]), Buffer.alloc(10), Buffer.alloc(13, 0x42), Buffer.from("mysql_native_password\0", "latin1"),
  ]);
  return Buffer.concat([Buffer.from([payload.length & 0xff, (payload.length >> 8) & 0xff, payload.length >> 16, 0]), payload]);
}

test("clears only the CLIENT_SSL bit of the server greeting", () => {
  const original = greeting(0xffff);
  const { packet, sslAdvertised } = stripSslCapability(original);
  assert.equal(sslAdvertised, true);
  const offset = capabilityOffset(original);
  assert.equal(packet.readUInt16LE(offset), 0xffff & ~CLIENT_SSL);
  const changed = [...packet].flatMap((byte, index) => (byte === original[index] ? [] : [index]));
  assert.deepEqual(changed, [offset + 1]);
  assert.equal(original.readUInt16LE(offset), 0xffff, "the input buffer is not modified");
});

test("reports a greeting that never advertised SSL", () => {
  const { packet, sslAdvertised } = stripSslCapability(greeting(0xf7ff));
  assert.equal(sslAdvertised, false);
  assert.equal(packet.readUInt16LE(capabilityOffset(packet)) & CLIENT_SSL, 0);
});

test("rejects anything that is not one complete HandshakeV10 packet", () => {
  const valid = greeting(0xffff);
  assert.throws(() => capabilityOffset(valid.subarray(0, 3)));
  assert.throws(() => capabilityOffset(valid.subarray(0, valid.length - 1)));
  assert.throws(() => capabilityOffset(Buffer.concat([valid, Buffer.from([0])])));
  const wrongSequence = Buffer.from(valid); wrongSequence[3] = 1;
  assert.throws(() => capabilityOffset(wrongSequence));
  const wrongProtocol = Buffer.from(valid); wrongProtocol[4] = 9;
  assert.throws(() => capabilityOffset(wrongProtocol));
});
