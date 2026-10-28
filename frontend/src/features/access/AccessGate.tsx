// Network deployments: the server asks for its access token before any API
// answers. The token is typed once and exchanged for an HttpOnly session
// cookie; the page never stores it. Local and loopback setups pass straight through.
import { useEffect, useState } from "react";
import type { FormEvent, JSX, ReactNode } from "react";
import { useLocale } from "../../i18n/locale";
import { AUTH_REQUIRED_EVENT, doctorApi, type DoctorApi } from "../../lib/api";
import "./access.css";

type GateState = "checking" | "open" | "locked";

export function AccessGate({ children, api = doctorApi }: { children: ReactNode; api?: DoctorApi }): JSX.Element {
  const [state, setState] = useState<GateState>("checking");
  useEffect(() => {
    let live = true;
    api.accessStatus()
      .then((status) => { if (live) setState(!status.required || status.authenticated ? "open" : "locked"); })
      // An older server without the endpoint is a local one: nothing to unlock.
      .catch(() => { if (live) setState("open"); });
    const lock = () => setState("locked");
    window.addEventListener(AUTH_REQUIRED_EVENT, lock);
    return () => { live = false; window.removeEventListener(AUTH_REQUIRED_EVENT, lock); };
  }, [api]);
  if (state === "open") return <>{children}</>;
  if (state === "checking") return <main className="access-page"><p className="workspace-loading" role="status">…</p></main>;
  return <AccessForm api={api} onOpen={() => setState("open")} />;
}

export function AccessForm({ api, onOpen }: { api: DoctorApi; onOpen(): void }): JSX.Element {
  const { text } = useLocale();
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!token.trim()) return;
    setBusy(true);
    setError(null);
    try {
      await api.login(token.trim());
      onOpen();
    } catch (cause) {
      const code = typeof cause === "object" && cause !== null && "code" in cause ? String(cause.code) : "request_failed";
      setError(code);
    } finally {
      setBusy(false);
      setToken("");
    }
  }
  const message = error === "auth_invalid" ? text("The access token is not correct.", "访问令牌不正确。")
    : error === "auth_throttled" ? text("Too many attempts. Try again in a minute.", "尝试次数过多，请一分钟后再试。")
    : error ? text(`Sign-in failed (${error}).`, `登录失败（${error}）。`) : null;
  return <main className="access-page">
    <form className="access-card" onSubmit={submit}>
      <h1>SR Doctor</h1>
      <p>{text("This server asks for its access token. Ask the person who deployed it.", "这台服务器需要访问令牌，请向部署它的人获取。")}</p>
      <label className="field-label">{text("Access token", "访问令牌")}
        <input autoComplete="current-password" autoFocus disabled={busy} onChange={(event) => setToken(event.target.value)} type="password" value={token} />
      </label>
      {message && <p className="access-error" role="alert">{message}</p>}
      <button className="button accent" disabled={busy || !token.trim()} type="submit">{busy ? text("Signing in…", "正在进入…") : text("Enter", "进入")}</button>
      <p className="access-note">{text("The token is exchanged for a browser session and is not stored by this page.", "令牌只用于换取浏览器会话，本页面不会保存它。")}</p>
    </form>
  </main>;
}
