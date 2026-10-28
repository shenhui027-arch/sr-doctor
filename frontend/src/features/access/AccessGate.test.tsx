import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { LocaleProvider } from "../../i18n/locale";
import { DoctorApi } from "../../lib/api";
import { AccessForm } from "./AccessGate";

function fakeApi(responses: Array<[number, unknown]>) {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const api = new DoctorApi("/api/v1", async (input, init) => {
    calls.push({ url: String(input), init });
    const [status, body] = responses.shift() ?? [500, null];
    return new Response(body === null ? null : JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
  });
  return { api, calls };
}

describe("account sign-in (network deployments)", () => {
  it("asks for the user name and password, with admin filled in", () => {
    const { api } = fakeApi([]);
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><AccessForm api={api} defaultAccount onOpen={() => undefined} /></LocaleProvider>);
    expect(html).toContain("用户名");
    expect(html).toContain('value="admin"');
    expect(html).toContain('type="password"');
    expect(html).toContain("admin / 123456");
  });

  it("shows the default account only while the server still uses it", () => {
    const { api } = fakeApi([]);
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><AccessForm api={api} defaultAccount={false} onOpen={() => undefined} /></LocaleProvider>);
    expect(html).not.toContain("123456");
    expect(html).toContain("向部署这台服务器的人获取");
  });

  it("reads the session status and posts the account as JSON once", async () => {
    const { api, calls } = fakeApi([[200, { required: true, authenticated: false, default_account: true }], [204, null]]);
    expect(await api.accessStatus()).toEqual({ required: true, authenticated: false, default_account: true });
    await api.login("admin", "123456");
    expect(calls[1].url).toBe("/api/v1/auth/session");
    expect(calls[1].init?.method).toBe("POST");
    expect(calls[1].init?.body).toBe(JSON.stringify({ username: "admin", password: "123456" }));
    expect(new Headers(calls[1].init?.headers).get("Content-Type")).toBe("application/json");
  });

  it("surfaces a wrong password as auth_invalid", async () => {
    const { api } = fakeApi([[401, { code: "auth_invalid", stage: "security", message_id: "auth_invalid", recoverability: "change_input", details: {} }]]);
    await expect(api.login("admin", "wrong")).rejects.toMatchObject({ code: "auth_invalid" });
  });
});
