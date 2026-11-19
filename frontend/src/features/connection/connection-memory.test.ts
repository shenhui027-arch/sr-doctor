import { afterEach, describe, expect, it, vi } from "vitest";
import { forgetResumableSession, readRememberedConnection, readResumableSession, rememberConnection, rememberResumableSession } from "./connection-memory";

function memoryStorage(): Storage {
  const values = new Map<string, string>();
  return {
    get length() { return values.size; },
    clear: () => values.clear(),
    getItem: (key) => values.get(key) ?? null,
    key: (index) => [...values.keys()][index] ?? null,
    removeItem: (key) => { values.delete(key); },
    setItem: (key, value) => { values.set(key, String(value)); },
  };
}

function stubWindow() {
  const local = memoryStorage();
  const session = memoryStorage();
  vi.stubGlobal("window", { localStorage: local, sessionStorage: session });
  return { local, session };
}

afterEach(() => vi.unstubAllGlobals());

describe("connection memory (task 71)", () => {
  it("remembers host, port, username and transport but never the password", () => {
    const { local } = stubWindow();
    rememberConnection({ host: " 10.0.0.5 ", port: 9030, username: "reader", password: "do-not-store", catalog: null, database: null, audit_table: null, tls_mode: "verify_full" });
    const raw = local.getItem("sr-doctor.connection") ?? "";
    expect(raw).not.toContain("do-not-store");
    expect(JSON.parse(raw)).toEqual({ host: "10.0.0.5", port: 9030, username: "reader", tls_mode: "verify_full" });
    expect(readRememberedConnection()).toEqual({ host: "10.0.0.5", port: 9030, username: "reader", tls_mode: "verify_full" });
  });

  it("ignores malformed or legacy remembered values", () => {
    const { local } = stubWindow();
    for (const value of ["not json", "null", '{"host":"h","port":0,"username":"u","tls_mode":"plaintext"}', '{"host":"h","port":9030,"username":"u","tls_mode":"docker_loopback_plaintext"}', '{"host":"","port":9030,"username":"u","tls_mode":"plaintext"}']) {
      local.setItem("sr-doctor.connection", value);
      expect(readRememberedConnection()).toBeNull();
    }
  });

  it("keeps the session ID per tab until it is read back and forgotten", () => {
    stubWindow();
    expect(readResumableSession()).toBeNull();
    rememberResumableSession("abc-123");
    expect(readResumableSession()).toBe("abc-123");
    forgetResumableSession();
    expect(readResumableSession()).toBeNull();
  });

  it("is a no-op when storage is unavailable", () => {
    vi.stubGlobal("window", { get localStorage(): Storage { throw new Error("blocked"); }, get sessionStorage(): Storage { throw new Error("blocked"); } });
    expect(readResumableSession()).toBeNull();
    expect(readRememberedConnection()).toBeNull();
    expect(() => { rememberResumableSession("x"); forgetResumableSession(); rememberConnection({ host: "h", port: 1, username: "u", password: "", catalog: null, database: null, audit_table: null, tls_mode: "plaintext" }); }).not.toThrow();
  });
});
