// Network deployments: the server asks for its login (default admin / 123456)
// before any API answers. The password is typed once and exchanged for an
// HttpOnly session cookie; the page never stores it. Local and loopback setups
// pass straight through.
import { useEffect, useState } from "react";
import type { FormEvent, JSX, ReactNode } from "react";
import { useLocale } from "../../i18n/locale";
import { AUTH_REQUIRED_EVENT, doctorApi, type DoctorApi } from "../../lib/api";
import "./access.css";

type GateState = "checking" | "open" | "locked";

export function AccessGate({ children, api = doctorApi }: { children: ReactNode; api?: DoctorApi }): JSX.Element {
  const [state, setState] = useState<GateState>("checking");
  const [defaultAccount, setDefaultAccount] = useState(false);
  useEffect(() => {
    let live = true;
    api.accessStatus()
      .then((status) => {
        if (!live) return;
        setDefaultAccount(status.default_account);
        setState(!status.required || status.authenticated ? "open" : "locked");
      })
      // An older server without the endpoint is a local one: nothing to unlock.
      .catch(() => { if (live) setState("open"); });
    const lock = () => setState("locked");
    window.addEventListener(AUTH_REQUIRED_EVENT, lock);
    return () => { live = false; window.removeEventListener(AUTH_REQUIRED_EVENT, lock); };
  }, [api]);
  if (state === "open") return <>{children}</>;
  if (state === "checking") return <main className="access-page"><p className="workspace-loading" role="status">…</p></main>;
  return <AccessForm api={api} defaultAccount={defaultAccount} onOpen={() => setState("open")} />;
}

export function AccessForm({ api, defaultAccount, onOpen }: { api: DoctorApi; defaultAccount: boolean; onOpen(): void }): JSX.Element {
  const { text } = useLocale();
  const [username, setUsername] = useState("admin");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!username.trim() || !password) return;
    setBusy(true);
    setError(null);
    try {
      await api.login(username.trim(), password);
      onOpen();
    } catch (cause) {
      const code = typeof cause === "object" && cause !== null && "code" in cause ? String(cause.code) : "request_failed";
      setError(code);
    } finally {
      setBusy(false);
      setPassword("");
    }
  }
  const message = error === "auth_invalid" ? text("The user name or password is not correct.", "用户名或密码不正确。")
    : error === "auth_throttled" ? text("Too many attempts. Try again in a minute.", "尝试次数过多，请一分钟后再试。")
    : error ? text(`Sign-in failed (${error}).`, `登录失败（${error}）。`) : null;
  return <main className="access-page">
    <form className="access-card" onSubmit={submit}>
      <h1>SR Doctor</h1>
      <p>{text("Sign in to use this server.", "登录后使用。")}</p>
      <label className="field-label">{text("User name", "用户名")}
        <input autoComplete="username" disabled={busy} onChange={(event) => setUsername(event.target.value)} value={username} />
      </label>
      <label className="field-label">{text("Password", "密码")}
        <input autoComplete="current-password" autoFocus disabled={busy} onChange={(event) => setPassword(event.target.value)} type="password" value={password} />
      </label>
      {message && <p className="access-error" role="alert">{message}</p>}
      <button className="button accent" disabled={busy || !username.trim() || !password} type="submit">{busy ? text("Signing in…", "正在登录…") : text("Sign in", "登录")}</button>
      <p className="access-note">{defaultAccount
        ? text("Default account: admin / 123456. Change it with DOCTOR_ADMIN_PASSWORD in .env.", "默认账号 admin / 123456，可在 .env 中用 DOCTOR_ADMIN_PASSWORD 修改。")
        : text("Ask the person who deployed this server for the password.", "密码请向部署这台服务器的人获取。")}</p>
    </form>
  </main>;
}
