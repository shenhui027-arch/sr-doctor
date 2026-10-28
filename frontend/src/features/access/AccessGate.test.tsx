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

describe("access token sign-in (network deployments)", () => {
  it("asks for the token without keeping it", () => {
    const { api } = fakeApi([]);
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><AccessForm api={api} onOpen={() => undefined} /></LocaleProvider>);
    expect(html).toContain("访问令牌");
    expect(html).toContain('type="password"');
    expect(html).toContain("本页面不会保存它");
  });

  it("reads the session status and posts the token as JSON once", async () => {
    const { api, calls } = fakeApi([[200, { required: true, authenticated: false }], [204, null]]);
    expect(await api.accessStatus()).toEqual({ required: true, authenticated: false });
    await api.login("correct-horse-battery-staple");
    expect(calls[1].url).toBe("/api/v1/auth/session");
    expect(calls[1].init?.method).toBe("POST");
    expect(calls[1].init?.body).toBe(JSON.stringify({ token: "correct-horse-battery-staple" }));
    expect(new Headers(calls[1].init?.headers).get("Content-Type")).toBe("application/json");
  });

  it("surfaces a wrong token as auth_invalid", async () => {
    const { api } = fakeApi([[401, { code: "auth_invalid", stage: "security", message_id: "auth_invalid", recoverability: "change_input", details: {} }]]);
    await expect(api.login("wrong")).rejects.toMatchObject({ code: "auth_invalid" });
  });
});
