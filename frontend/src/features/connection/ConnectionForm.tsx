import { useEffect, useMemo, useRef, useState } from "react";
import type { Dispatch, FormEvent, JSX, RefObject } from "react";
import type { ConnectionCreateRequest, SavedConnectionUnavailableReason, SavedConnectionView } from "../../../../contracts/generated/doctor-contracts";
import { useLocale } from "../../i18n/locale";
import { doctorApi, type DoctorApi } from "../../lib/api";
import { InfoTip } from "../shell/ui";
import { CapabilityList } from "./CapabilityList";
import { readRememberedConnection, rememberConnection } from "./connection-memory";
import { ConnectionStateView, connectionFailureMessage, isConnectionExpiredFailure } from "./ConnectionDialog";
import "./connection-form.css";
import { connectionDraftError, type ConnectionAction, type ConnectionDraft, type ConnectionState, type TlsMode } from "./connection-state";

// Like a desktop SQL client: type the host, port, user and password; plaintext
// unless the cluster has SSL enabled and the user picks TLS verify-full.
const emptyDraft: ConnectionCreateRequest = {
  host: "",
  port: 9030,
  username: "",
  password: "",
  catalog: null,
  database: null,
  audit_table: null,
  tls_mode: "plaintext",
};

/**
 * Read-only connection fields, probe and close. Used inside the connection
 * dialog and on the first-run page. The draft (and its password) lives only
 * in this component, so unmounting it discards the credentials.
 */
export function ConnectionForm({ api = doctorApi, dispatch, hostInputRef, idPrefix, onDone, onSessionEnded, onSessionExpired, showCapabilities = false, state, submitLabel }: {
  api?: DoctorApi;
  dispatch: Dispatch<ConnectionAction>;
  hostInputRef?: RefObject<HTMLInputElement | null>;
  idPrefix: string;
  onDone?(): void;
  onSessionEnded?(): void;
  onSessionExpired?(): void;
  /** First run (design board 05): list the capability probe and drop the idle hint. */
  showCapabilities?: boolean;
  state: ConnectionState;
  submitLabel?: string;
}): JSX.Element {
  const { text } = useLocale();
  // Task 71: the last successful host, port, username and transport come back filled in (never the password).
  const [draft, setDraft] = useState<ConnectionCreateRequest>(() => ({ ...emptyDraft, ...readRememberedConnection() }));
  const validationError = useMemo(() => connectionDraftError(draft), [draft]);
  // Task 72 (design C1): the connection saved on the server, and whether to save this one.
  const [saved, setSaved] = useState<SavedConnectionView | null>(null);
  const [saveCredentials, setSaveCredentials] = useState(false);
  const touchedRef = useRef(false);
  useEffect(() => {
    const controller = new AbortController();
    api.savedConnection(controller.signal).then((view) => {
      if (controller.signal.aborted) return;
      setSaved(view);
      if (view.state !== "saved" && view.state !== "needs_reentry") return;
      setSaveCredentials(true);
      const { host, port, username, tls_mode: tlsMode } = view;
      if (!touchedRef.current && host && port && username && tlsMode) setDraft((current) => ({ ...current, host, port, username, tls_mode: tlsMode }));
    }).catch(() => undefined);
    return () => controller.abort();
  }, [api]);
  const saveBlocked = saved === null || saved.state === "unavailable";
  const connectControllerRef = useRef<AbortController | null>(null);
  const doneButtonRef = useRef<HTMLButtonElement>(null);

  // A fresh successful probe moves focus to Done so Enter closes instead of reconnecting.
  useEffect(() => {
    if (onDone && state.phase === "ready") doneButtonRef.current?.focus();
  }, [onDone, state.phase]);

  useEffect(() => () => {
    connectControllerRef.current?.abort();
    connectControllerRef.current = null;
  }, []);

  function update<K extends keyof ConnectionDraft>(key: K, value: ConnectionDraft[K]) {
    touchedRef.current = true;
    setDraft((current) => ({ ...current, [key]: value }));
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (validationError) {
      dispatch({ type: "fail", message: validationError });
      update("password", "");
      return;
    }
    dispatch({ type: "connect" });
    connectControllerRef.current?.abort();
    const controller = new AbortController();
    connectControllerRef.current = controller;
    try {
      const save = saveCredentials && !saveBlocked;
      const session = await api.createConnection(save ? { ...draft, save_credentials: true } : draft, controller.signal);
      if (controller.signal.aborted) return;
      rememberConnection(draft);
      if (save) setSaved((current) => current && { ...current, state: "saved", host: draft.host.trim(), port: draft.port, username: draft.username.trim(), tls_mode: draft.tls_mode });
      dispatch({ type: "session", session });
    } catch (error) {
      if (!controller.signal.aborted) dispatch({ type: "fail", message: connectionFailureMessage(error, "Connection failed.") });
    } finally {
      if (connectControllerRef.current === controller) connectControllerRef.current = null;
      update("password", "");
    }
  }

  async function closeSession() {
    try {
      if ((state.phase === "ready" || state.phase === "unsupported") && state.session) {
        await api.closeConnection(state.session.connection_id);
      }
      onSessionEnded?.();
      dispatch({ type: "close" });
    } catch (error) {
      if (isConnectionExpiredFailure(error)) {
        if (onSessionExpired) onSessionExpired();
        else dispatch({ type: "expire", message: "connection_expired" });
      }
      else dispatch({ type: "fail", message: connectionFailureMessage(error, "Connection close failed.") });
    } finally {
      update("password", "");
    }
  }

  const tlsOptions: Array<[TlsMode, string, string]> = [
    ["plaintext", text("Plaintext", "明文"), text("Any reachable host; password and SQL are not encrypted", "任意可达主机；口令与 SQL 不加密传输")],
    ["verify_full", "TLS verify-full", text("Encrypted with a verified certificate; needs SSL on the FE", "加密并校验证书；需要 FE 开启 SSL")],
  ];
  return <>
    <form aria-busy={state.phase === "connecting"} className="connection-form" onSubmit={submit}>
      <div className="connection-fields">
        <label className="field-label">{text("Host", "主机")}<input autoComplete="off" className="mono-input" onChange={(event) => update("host", event.target.value)} placeholder={text("FE address, e.g. 10.0.0.5", "FE 地址，如 10.0.0.5")} ref={hostInputRef} value={draft.host} /></label>
        <label className="field-label">{text("Port", "端口")}<input className="mono-input" inputMode="numeric" onChange={(event) => update("port", Number(event.target.value))} value={draft.port} /></label>
        <label className="field-label">{text("Username", "用户名")}<input autoComplete="username" onChange={(event) => update("username", event.target.value)} value={draft.username} /></label>
        <label className="field-label">{text("Password", "密码")}<input autoComplete="off" onChange={(event) => update("password", event.target.value)} type="password" value={draft.password} /></label>
      </div>
      <fieldset className="tls-options">
        <legend>{text("Transport security", "传输安全")}</legend>
        {tlsOptions.map(([mode, label, hint]) => <label className={draft.tls_mode === mode ? "selected" : ""} key={mode}>
          <input checked={draft.tls_mode === mode} name={`${idPrefix}-tls`} onChange={() => update("tls_mode", mode)} type="radio" value={mode} />
          <span><strong>{label}</strong><small>{hint}</small></span>
        </label>)}
      </fieldset>
      <div className="connection-save">
        <label>
          <input checked={saveCredentials && !saveBlocked} disabled={saveBlocked} onChange={(event) => setSaveCredentials(event.target.checked)} type="checkbox" />
          <span>{text("Save the password encrypted on the SR Doctor server and keep collecting", "在 SR Doctor 服务端加密保存口令，持续采集")}</span>
        </label>
        <InfoTip label={text("About saving the password", "关于保存口令")}>{text("The password is sealed with AES-256-GCM in the history store of the server running SR Doctor (not in this browser): write-only, never returned to the page or logged. The page then connects on open, and anyone who can sign in to SR Doctor can use it to reach the cluster, so prefer a read-only account.", "口令用 AES-256-GCM 加密存进运行 SR Doctor 的服务端历史库（不在浏览器里），只写不读，不返回页面、不写日志。保存后页面打开时自动连接；能登录 SR Doctor 的人都能借它连接集群，建议使用只读账号。")}</InfoTip>
      </div>
      {saved?.state === "unavailable" && saved.unavailable_reason ? <p className="connection-save-note">{saveUnavailableText(saved.unavailable_reason, text)}</p> : null}
      {saved?.state === "needs_reentry" ? <p className="connection-save-note warn">{text("The saved password no longer works; enter it again to save it.", "已保存的口令已失效，请重新输入后保存。")}</p> : null}
      {saveCredentials && !saveBlocked ? <p className="connection-note">{text("Collection keeps running after this page is closed. Forget the saved connection under Settings › Connection.", "关闭网页后仍会采集；可在 设置 › 连接 里忘记已保存的连接。")}</p> : <p className="connection-note">{text("Credentials stay in this process's short-lived session only: never written to disk, returned to the page or logged, and deleted on disconnect or expiry. This browser remembers the host, port and username, never the password.", "凭据只保存在本进程的短期会话中，不落盘、不返回页面、不写日志；断开或过期即删除。本浏览器只记住主机、端口和用户名，不记口令。")}</p>}
      <div className="row actions connection-actions">
        {onDone && state.phase === "ready" && <button className="button accent" onClick={onDone} ref={doneButtonRef} type="button">{text("Done", "完成")}</button>}
        {(state.phase === "ready" || state.phase === "unsupported") && <button className="button secondary" onClick={closeSession} title={saved?.state === "saved" ? text("Background collection with the saved connection keeps running.", "已保存连接的后台采集会继续。") : undefined} type="button">{saved?.state === "saved" ? text("Disconnect this page", "断开本页连接") : text("Disconnect", "断开连接")}</button>}
        <button className={state.phase === "ready" ? "button secondary" : "button dark"} disabled={state.phase === "connecting"} type="submit">{state.phase === "connecting" ? text("Probing…", "探测中…") : submitLabel ?? text("Connect read-only", "只读连接")}</button>
      </div>
    </form>
    {showCapabilities && (state.phase === "closed" && !state.message) ? null : <ConnectionStateView state={state} />}
    {showCapabilities && <CapabilityList state={state} />}
  </>;
}

export function saveUnavailableText(reason: SavedConnectionUnavailableReason, text: (en: string, zh: string) => string): string {
  switch (reason) {
    case "history_unavailable": return text("Saving needs the history store, which is off.", "历史库未开启，无法保存口令。");
    case "key_unavailable": return text("Saving needs a master key (DOCTOR_SECRET_KEY or secret.key).", "没有主密钥（DOCTOR_SECRET_KEY 或 secret.key），无法保存口令。");
    case "default_admin_password": return text("This network deployment still uses the default login password; change DOCTOR_ADMIN_PASSWORD in .env before saving.", "网络模式仍在使用默认登录密码，先在 .env 修改 DOCTOR_ADMIN_PASSWORD 才能保存口令。");
  }
}
