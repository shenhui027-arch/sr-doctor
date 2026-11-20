import { useEffect, useMemo, useRef, useState } from "react";
import type { Dispatch, FormEvent, JSX, RefObject } from "react";
import type { ConnectionCreateRequest, SavedConnectionUnavailableReason, SavedConnectionView } from "../../../../contracts/generated/doctor-contracts";
import { Button, InfoTip } from "../../design";
import { lookupEnumLabel } from "../../i18n/enum-label";
import { useLocale } from "../../i18n/locale";
import { doctorApi, type DoctorApi } from "../../lib/api";
import { CapabilityList } from "./CapabilityList";
import { readRememberedConnection, rememberConnection } from "./connection-memory";
import { ConnectionStateView, connectionFailureMessage, isConnectionExpiredFailure } from "./ConnectionDialog";
import "./connection-form.css";
import { connectionDraftError, type ConnectionAction, type ConnectionDraft, type ConnectionState, type TlsMode } from "./connection-state";

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

const SAVE_UNAVAILABLE_LABELS: Record<SavedConnectionUnavailableReason, readonly [string, string]> = {
  history_unavailable: ["Saving needs the history store, which is off.", "历史库未开启，无法保存口令。"],
  key_unavailable: ["Saving needs a master key (DOCTOR_SECRET_KEY or secret.key).", "没有主密钥（DOCTOR_SECRET_KEY 或 secret.key），无法保存口令。"],
  default_admin_password: [
    "This network deployment still uses the default login password; change DOCTOR_ADMIN_PASSWORD in .env before saving.",
    "网络模式仍在使用默认登录密码，先在 .env 修改 DOCTOR_ADMIN_PASSWORD 才能保存口令。",
  ],
};

/**
 * Read-only connection fields, probe and close. Used inside the connection
 * dialog and on the first-run page. The draft (and its password) lives only
 * in this component, so unmounting it discards the credentials.
 */
export function ConnectionForm({
  api = doctorApi,
  dispatch,
  hostInputRef,
  idPrefix,
  onDone,
  onSessionEnded,
  onSessionExpired,
  renderLayout,
  showCapabilities = false,
  state,
  submitLabel,
}: {
  api?: DoctorApi;
  dispatch: Dispatch<ConnectionAction>;
  hostInputRef?: RefObject<HTMLInputElement | null>;
  idPrefix: string;
  onDone?(): void;
  onSessionEnded?(): void;
  onSessionExpired?(): void;
  /** Places `body` and `footer` inside a `Card` or `Dialog` while keeping credentials inside this component. */
  renderLayout?(parts: { body: JSX.Element; footer: JSX.Element }): JSX.Element;
  showCapabilities?: boolean;
  state: ConnectionState;
  submitLabel?: string;
}): JSX.Element {
  const { text } = useLocale();
  const [draft, setDraft] = useState<ConnectionCreateRequest>(() => ({ ...emptyDraft, ...readRememberedConnection() }));
  const validationError = useMemo(() => connectionDraftError(draft), [draft]);
  const [saved, setSaved] = useState<SavedConnectionView | null>(null);
  const [saveCredentials, setSaveCredentials] = useState(false);
  const touchedRef = useRef(false);
  const formId = `${idPrefix}-connection-form`;
  const boundaryId = idPrefix === "dialog" ? "connection-boundary" : `${idPrefix}-connection-boundary`;

  useEffect(() => {
    const controller = new AbortController();
    api.savedConnection(controller.signal).then((view) => {
      if (controller.signal.aborted) return;
      setSaved(view);
      if (view.state !== "saved" && view.state !== "needs_reentry") return;
      setSaveCredentials(true);
      const { host, port, username, tls_mode: tlsMode } = view;
      if (!touchedRef.current && host && port && username && tlsMode) {
        setDraft((current) => ({ ...current, host, port, username, tls_mode: tlsMode }));
      }
    }).catch(() => undefined);
    return () => controller.abort();
  }, [api]);

  const saveBlocked = saved === null || saved.state === "unavailable";
  const connectControllerRef = useRef<AbortController | null>(null);
  const doneButtonRef = useRef<HTMLButtonElement>(null);

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
      if (save) {
        setSaved((current) => current && {
          ...current,
          state: "saved",
          host: draft.host.trim(),
          port: draft.port,
          username: draft.username.trim(),
          tls_mode: draft.tls_mode,
        });
      }
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
      } else {
        dispatch({ type: "fail", message: connectionFailureMessage(error, "Connection close failed.") });
      }
    } finally {
      update("password", "");
    }
  }

  const tlsModes: Array<[TlsMode, string]> = [
    ["plaintext", text("Plaintext", "明文连接")],
    ["verify_full", "TLS verify-full"],
  ];

  const isPlaintext = draft.tls_mode !== "verify_full";

  const footer = (
    <>
      {onDone && state.phase === "ready" ? (
        <button className="btn btn-primary" onClick={onDone} ref={doneButtonRef} type="button">
          {text("Done", "完成")}
        </button>
      ) : null}
      {(state.phase === "ready" || state.phase === "unsupported") ? (
        <Button
          onClick={closeSession}
          title={saved?.state === "saved" ? text("Background collection with the saved connection keeps running.", "已保存连接的后台采集会继续。") : undefined}
          variant="secondary"
        >
          {saved?.state === "saved" ? text("Disconnect this page", "断开本页连接") : text("Disconnect", "断开连接")}
        </Button>
      ) : null}
      <Button
        disabled={state.phase === "connecting"}
        form={formId}
        type="submit"
        variant={state.phase === "ready" ? "secondary" : "primary"}
      >
        {state.phase === "connecting" ? text("Probing…", "探测中…") : submitLabel ?? text("Connect read-only", "只读连接")}
      </Button>
    </>
  );

  const body = (
    <div className="connection-form-stack">
      <p className="card-subtitle connection-boundary-note" id={boundaryId}>
        {text(
          "Read-only connection by default (blocks multi-statements, DDL, DML, EXPLAIN ANALYZE and Plan Advisor mutations); Runtime Top is opt-in per query with a fixed 10-second timeout and discards result rows.",
          "默认只读连接（禁止多语句、DDL、DML、EXPLAIN ANALYZE 与 Plan Advisor 修改）；Runtime Top 需显式开启，超时固定为 10 秒并丢弃结果行。",
        )}
      </p>
      <form aria-busy={state.phase === "connecting"} aria-describedby={boundaryId} className="connection-form" id={formId} onSubmit={submit}>
        <div className="connection-host-grid">
          <div className="field">
            <label className="field-label" htmlFor={`${idPrefix}-host`}>{text("FE host", "FE 主机")}</label>
            <input
              autoComplete="off"
              className="input mono"
              id={`${idPrefix}-host`}
              onChange={(event) => update("host", event.target.value)}
              placeholder={text("e.g. 10.0.0.5", "如 10.0.0.5")}
              ref={hostInputRef}
              type="text"
              value={draft.host}
            />
          </div>
          <div className="field">
            <label className="field-label" htmlFor={`${idPrefix}-port`}>{text("Port", "端口")}</label>
            <input
              className="input mono"
              id={`${idPrefix}-port`}
              inputMode="numeric"
              onChange={(event) => update("port", Number(event.target.value))}
              type="number"
              value={draft.port}
            />
          </div>
        </div>

        <div className="connection-auth-grid">
          <div className="field">
            <label className="field-label" htmlFor={`${idPrefix}-user`}>{text("Username", "用户名")}</label>
            <input
              autoComplete="username"
              className="input mono"
              id={`${idPrefix}-user`}
              onChange={(event) => update("username", event.target.value)}
              type="text"
              value={draft.username}
            />
          </div>
          <div className="field">
            <label className="field-label" htmlFor={`${idPrefix}-pass`}>{text("Password", "口令")}</label>
            <input
              autoComplete="off"
              className="input mono"
              id={`${idPrefix}-pass`}
              onChange={(event) => update("password", event.target.value)}
              type="password"
              value={draft.password}
            />
          </div>
        </div>

        <fieldset className="tls-fieldset">
          <legend className="field-label">{text("Transport security", "传输安全")}</legend>
          <div className="seg-tabs tls-seg-tabs">
            {tlsModes.map(([mode, label]) => {
              const active = draft.tls_mode === mode || (mode === "plaintext" && draft.tls_mode === "docker_loopback_plaintext");
              return (
                <label className={active ? "seg-tab is-active" : "seg-tab"} key={mode}>
                  <input
                    checked={active}
                    className="sr-only"
                    name={`${idPrefix}-tls`}
                    onChange={() => update("tls_mode", mode)}
                    type="radio"
                    value={mode}
                  />
                  <span>{label}</span>
                </label>
              );
            })}
          </div>
        </fieldset>

        {isPlaintext ? (
          <div className="notice-banner notice-banner--warn" role="status">
            <span>
              <strong>{text("Security notice: password and SQL are not encrypted in transit.", "安全提示：口令与 SQL 不加密传输。")}</strong>{" "}
              {text("Enable TLS verify-full across untrusted networks (full certificate verification, never downgraded).", "跨不可信网络请启用 TLS verify-full（完整校验证书，不降级）。")}
            </span>
          </div>
        ) : (
          <div className="notice-banner" role="status">
            <span>
              <strong>TLS verify-full:</strong>{" "}
              {text("Encrypts traffic and verifies the FE certificate and hostname.", "加密传输并完整校验 FE 证书与主机名。")}
            </span>
          </div>
        )}

        <div className="connection-save-row">
          <label className="check-inline">
            <input
              checked={saveCredentials && !saveBlocked}
              disabled={saveBlocked}
              onChange={(event) => setSaveCredentials(event.target.checked)}
              type="checkbox"
            />
            <span>{text("Save the password encrypted on the SR Doctor server and keep collecting", "在服务端加密保存口令，持续后台采集")}</span>
          </label>
          <InfoTip label={text("About saving the password", "加密保存连接边界说明")}>
            {text(
              "AES-256-GCM write-only in the server history store (at most 1 saved connection; never returned or logged). Disabled when network mode still uses the default login password; deleted immediately when forgotten or when history is cleared.",
              "口令用 AES-256-GCM 只写不读存入服务端历史库（最多保存 1 个连接，永不回显或写日志）。网络模式仍使用默认登录密码时拒绝保存；忘记连接或清空历史时立即删除。默认不勾选时凭据只保存在本进程的短期会话中，断开或过期即删除。",
            )}
          </InfoTip>
        </div>

        {saved?.state === "unavailable" && saved.unavailable_reason ? (
          <div className="notice-banner" role="note">
            <span>{saveUnavailableText(saved.unavailable_reason, text)}</span>
          </div>
        ) : null}
        {saved?.state === "needs_reentry" ? (
          <div className="notice-banner notice-banner--warn" role="status">
            <span>{text("The saved password no longer works; enter it again to save it.", "已保存的口令已失效，请重新输入后保存。")}</span>
          </div>
        ) : null}

        <details className="disclosure">
          <summary>{text("Optional scope (Catalog / Default database / Audit table)", "可选范围（Catalog / 默认库 / 审计表）")}</summary>
          <div className="connection-optional-grid">
            <div className="field">
              <label className="field-label" htmlFor={`${idPrefix}-cat`}>Catalog</label>
              <input
                className="input mono"
                id={`${idPrefix}-cat`}
                onChange={(event) => update("catalog", event.target.value.trim() || null)}
                placeholder="default_catalog"
                type="text"
                value={draft.catalog ?? ""}
              />
            </div>
            <div className="field">
              <label className="field-label" htmlFor={`${idPrefix}-db`}>{text("Default database", "默认库")}</label>
              <input
                className="input mono"
                id={`${idPrefix}-db`}
                onChange={(event) => update("database", event.target.value.trim() || null)}
                placeholder="tpch_100g"
                type="text"
                value={draft.database ?? ""}
              />
            </div>
            <div className="field">
              <label className="field-label" htmlFor={`${idPrefix}-audit`}>{text("Audit table", "审计表")}</label>
              <input
                className="input mono"
                id={`${idPrefix}-audit`}
                onChange={(event) => update("audit_table", event.target.value.trim() || null)}
                placeholder="starrocks_audit_db__.starrocks_audit_tbl__"
                type="text"
                value={draft.audit_table ?? ""}
              />
            </div>
          </div>
        </details>
      </form>

      {showCapabilities && state.phase === "closed" && !state.message ? null : <ConnectionStateView state={state} />}
      {showCapabilities ? <CapabilityList state={state} /> : null}
    </div>
  );

  if (renderLayout) return renderLayout({ body, footer });
  return (
    <>
      {body}
      <div className="connection-footer-actions connection-inline-actions">{footer}</div>
    </>
  );
}

export function saveUnavailableText(reason: SavedConnectionUnavailableReason, text: (en: string, zh: string) => string): string {
  return lookupEnumLabel(SAVE_UNAVAILABLE_LABELS, reason, text);
}
