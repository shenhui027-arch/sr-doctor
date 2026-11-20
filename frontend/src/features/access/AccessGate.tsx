// Network deployments: the server asks for its login (default admin / 123456)
// before any API answers. The password is typed once and exchanged for an
// HttpOnly session cookie; the page never stores it. Local and loopback setups
// pass straight through.
import { useEffect, useState } from "react";
import type { FormEvent, JSX, ReactNode } from "react";
import { Button, Card, InfoTip } from "../../design";
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
  const formId = "access-login-form";

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

  const message = error === "auth_invalid"
    ? text("Sign-in failed: the user name or password is not correct.", "登录失败：用户名或密码不正确。")
    : error === "auth_throttled"
      ? text("Too many attempts. Wait 1 minute and try again.", "请求受限：连续尝试次数过多，请等待 1 分钟后再试。")
      : error
        ? text("Sign-in failed. Check the server connection and try again.", "登录失败，请检查服务连接后重试。")
        : null;

  return (
    <main className="access-page">
      <Card
        className="access-login-card"
        footer={
          <>
            <span className="mono">{text("Network mode", "网络模式")}</span>
            <Button
              disabled={busy || !username.trim() || !password}
              form={formId}
              type="submit"
              variant="primary"
            >
              {busy ? text("Signing in…", "正在登录…") : text("Sign in", "登录")}
            </Button>
          </>
        }
        scroll={false}
        title={text("Sign in to SR Doctor", "登录 SR Doctor")}
        titleExtra={
          <InfoTip label={text("Session cookie details", "会话 Cookie 说明")}>
            {text(
              "The password is used only to obtain an HttpOnly, SameSite=Strict session cookie; it is never echoed or written to logs.",
              "密码仅用于换取 HttpOnly、SameSite=Strict 会话 Cookie，不回显、不写日志。",
            )}
          </InfoTip>
        }
        variant="form"
      >
        <form className="access-form" id={formId} onSubmit={submit}>
          {defaultAccount ? (
            <div className="notice-banner notice-banner--warn" role="status">
              <span>
                <strong>{text("Default public password:", "默认公开口令：")}</strong>{" "}
                {text(
                  "Still using admin / 123456. Set DOCTOR_ADMIN_PASSWORD in .env before exposing this server to an untrusted network; saving StarRocks passwords is disabled while the default password is active.",
                  "当前仍使用 admin / 123456。暴露到不可信网络前请在 .env 设置 DOCTOR_ADMIN_PASSWORD；默认密码下服务端拒绝保存 StarRocks 连接口令。",
                )}
              </span>
            </div>
          ) : (
            <div className="notice-banner" role="note">
              <span>{text("Ask the person who deployed this server for the password.", "密码请向部署这台服务器的人获取。")}</span>
            </div>
          )}

          {message ? (
            <div className="notice-banner notice-banner--danger" role="alert">
              <span>{message}</span>
            </div>
          ) : null}

          <div className="field">
            <label className="field-label" htmlFor="login-user">{text("User name", "用户名")}</label>
            <input
              autoComplete="username"
              className="input mono"
              disabled={busy}
              id="login-user"
              onChange={(event) => setUsername(event.target.value)}
              type="text"
              value={username}
            />
          </div>

          <div className="field">
            <label className="field-label" htmlFor="login-pass">{text("Password", "密码")}</label>
            <input
              autoComplete="current-password"
              autoFocus
              className="input mono"
              disabled={busy}
              id="login-pass"
              onChange={(event) => setPassword(event.target.value)}
              type="password"
              value={password}
            />
          </div>
        </form>
      </Card>
    </main>
  );
}
