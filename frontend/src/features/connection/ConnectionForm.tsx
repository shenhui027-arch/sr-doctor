import { useEffect, useMemo, useRef, useState } from "react";
import type { Dispatch, FormEvent, JSX, RefObject } from "react";
import type { ConnectionCreateRequest } from "../../../../contracts/generated/doctor-contracts";
import { useLocale } from "../../i18n/locale";
import { doctorApi, type DoctorApi } from "../../lib/api";
import { CapabilityList } from "./CapabilityList";
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
  const [draft, setDraft] = useState(emptyDraft);
  const validationError = useMemo(() => connectionDraftError(draft), [draft]);
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
      const session = await api.createConnection(draft, controller.signal);
      if (controller.signal.aborted) return;
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
      <p className="connection-note">{text("Credentials stay in this process's short-lived session only: never written to disk, returned to the page or logged, and deleted on disconnect or expiry.", "凭据只保存在本进程的短期会话中，不落盘、不返回页面、不写日志；断开或过期即删除。")}</p>
      <div className="row actions connection-actions">
        {onDone && state.phase === "ready" && <button className="button accent" onClick={onDone} ref={doneButtonRef} type="button">{text("Done", "完成")}</button>}
        {(state.phase === "ready" || state.phase === "unsupported") && <button className="button secondary" onClick={closeSession} type="button">{text("Disconnect", "断开连接")}</button>}
        <button className={state.phase === "ready" ? "button secondary" : "button dark"} disabled={state.phase === "connecting"} type="submit">{state.phase === "connecting" ? text("Probing…", "探测中…") : submitLabel ?? text("Connect read-only", "只读连接")}</button>
      </div>
    </form>
    {showCapabilities && (state.phase === "closed" && !state.message) ? null : <ConnectionStateView state={state} />}
    {showCapabilities && <CapabilityList state={state} />}
  </>;
}
