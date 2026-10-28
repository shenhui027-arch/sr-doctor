import { useCallback, useEffect, useRef } from "react";
import type { Dispatch } from "react";
import type { JSX } from "react";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { doctorApi, type DoctorApi } from "../../lib/api";
import type { ConnectionAction, ConnectionState } from "./connection-state";
import { ConnectionForm } from "./ConnectionForm";

export function ConnectionDialog({ open, onClose, onSessionEnded, onSessionExpired, state, dispatch, api = doctorApi }: { open: boolean; onClose(): void; onSessionEnded?(): void; onSessionExpired?(): void; state: ConnectionState; dispatch: Dispatch<ConnectionAction>; api?: DoctorApi }): JSX.Element | null {
  const { text } = useLocale();
  const dialogRef = useRef<HTMLElement>(null);
  const hostInputRef = useRef<HTMLInputElement>(null);

  const dismiss = useCallback(() => onClose(), [onClose]);

  useEffect(() => {
    if (!open) return;
    const dialog = dialogRef.current;
    if (!dialog) return;
    hostInputRef.current?.focus();
    const focusable = () => Array.from(dialog.querySelectorAll<HTMLElement>("button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])"));
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        dismiss();
        return;
      }
      if (event.key !== "Tab") return;
      const items = focusable();
      if (items.length === 0) return;
      const first = items[0];
      const last = items[items.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    dialog.addEventListener("keydown", onKeyDown);
    return () => dialog.removeEventListener("keydown", onKeyDown);
  }, [dismiss, open]);

  if (!open) return null;
  return (
    <div className="dialog-backdrop" role="presentation">
      <section className="dialog" aria-describedby="connection-boundary" aria-labelledby="connection-title" aria-modal="true" ref={dialogRef} role="dialog">
        <div className="panel-heading">
          <div>
            <h2 id="connection-title">{text("Connect StarRocks", "连接 StarRocks")}</h2>
            <span className="chip accent">{text("Read-only connection", "只读连接")}</span>
          </div>
          <button className="button secondary" onClick={dismiss} type="button">{text("Close", "关闭")}</button>
        </div>
        <p className="warning" id="connection-boundary">{text("Topology mode uses fixed EXPLAIN COSTS. Runtime Top is an explicit opt-in that executes one SELECT/WITH with a 10-second timeout and discards result rows. DDL, DML, EXPLAIN ANALYZE and Plan Advisor mutations remain blocked.", "仅拓扑模式固定使用 EXPLAIN COSTS。Runtime Top 需显式开启，仅执行一条 SELECT/WITH，超时固定为 10 秒并丢弃结果行；DDL、DML、EXPLAIN ANALYZE 和 Plan Advisor 修改仍被禁止。")}</p>
        <ConnectionForm api={api} dispatch={dispatch} hostInputRef={hostInputRef} idPrefix="dialog" onDone={dismiss} onSessionEnded={onSessionEnded} onSessionExpired={onSessionExpired} state={state} />
      </section>
    </div>
  );
}

export function ConnectionStateView({ state }: { state: ConnectionState }): JSX.Element {
  const { text } = useLocale();
  if ((state.phase === "ready" || state.phase === "unsupported") && state.session) {
    return <div aria-live="polite" className={`connection-state ${state.phase === "ready" ? "success" : "warning"}`} role="status"><strong>{state.phase === "ready" ? text("Connected", "已连接") : text("Connected with limited support", "已连接，部分功能受限")}</strong><span>{text("Version", "版本")} {state.session.detected_version?.value ?? text("Unknown", "未知")}</span>{state.phase === "unsupported" && <span>{text("This version is not supported for analysis. Check version compatibility before continuing.", "当前版本暂不支持分析，请先核对版本兼容性。")}</span>}</div>;
  }
  if (state.phase === "connecting") return <div className="connection-state" role="status">{text("Connecting…", "连接中…")}</div>;
  if (state.phase === "expired") return <div className="connection-state warning" role="alert">{state.message === "remote_timeout"
    ? text("A read timed out and the server closed this connection. Reconnect to continue.", "一次读取超时，服务端已关闭此连接，请重新连接。")
    : text("Connection expired. Reconnect to continue.", "连接已过期，请重新连接。")}</div>;
  if (state.phase === "failed") {
    const message = localizedConnectionMessage(state.message, text);
    return <div className="connection-state error" role="alert">{message && message !== state.message ? message : text("Connection failed. Check the connection details and network, then retry.", "连接失败，请检查连接信息与网络后重试。")}</div>;
  }
  if (state.phase === "closed" && state.message) return <div aria-live="polite" className="connection-state" role="status">{localizedConnectionMessage(state.message, text)}</div>;
  return <div aria-live="polite" className="connection-state" role="status">{text("Not connected. Enter connection details to continue.", "尚未连接，请填写连接信息。")}</div>;
}

export function localizedConnectionMessage(message: string | null | undefined, text: LocalizedText): string | null {
  if (!message) return null;
  const known: Record<string, [string, string]> = {
    connection_expired: ["The local connection session expired. Reconnect to continue.", "本地连接会话已过期，请重新连接。"],
    connection_session_expired: ["The connection expired. Reconnect to continue.", "连接已过期，请重新连接。"],
    authentication_failed: ["StarRocks rejected the supplied credentials.", "用户名或密码未通过验证，请检查后重试。"],
    permission_denied: ["The connection lacks a required read-only permission.", "当前连接缺少必需的只读权限。"],
    tls_failed: ["TLS verify-full failed. Check the server name and certificate chain.", "TLS 完整校验失败，请检查服务器名称与证书链。"],
    connection_failed: ["The StarRocks connection failed.", "连接失败，请检查主机、端口与网络后重试。"],
    invalid_request: ["The connection request is invalid.", "连接信息无效，请检查必填项与端口。"],
    tls_required: ["TLS verify-full is required for non-loopback hosts.", "非回环主机必须使用 TLS 完整校验。"],
    capability_unavailable: ["A required read-only capability is unavailable.", "必需的只读能力不可用。"],
    unsupported_version: ["This StarRocks release is not supported: 3.1 and later releases covered by the source audit (3.1–4.1) are.", "不支持当前 StarRocks 版本：支持 3.1 及以后、已经过源码审计的版本（3.1～4.1）。"],
    profile_unavailable: ["The requested Profile is unavailable.", "请求的 Profile 不可用。"],
    statement_not_allowed: ["The statement is outside the read-only product boundary.", "该语句超出产品只读边界。"],
    explain_rejected: ["StarRocks rejected the Explain request.", "StarRocks 拒绝了计划检查请求。"],
    remote_timeout: ["The StarRocks request timed out.", "连接请求超时，请检查网络和服务状态。"],
    session_limit: ["Too many open connections (3 at most). Disconnect one you no longer use, or wait a few minutes for idle ones to expire.", "同时打开的连接已达上限（最多 3 个）。请断开不用的连接，或等几分钟让空闲连接自动过期。"],
    "Connection closed; credentials were removed from the server session.": ["Connection closed; credentials were removed from the server session.", "连接已关闭，凭据已从服务端会话移除。"],
    "Host, port, username, and password are required.": ["Host, port, username, and password are required.", "主机、端口、用户名和密码均为必填项。"],
    "Plaintext is available only for a loopback Docker lab.": ["Plaintext is available only for a loopback Docker lab.", "明文连接仅允许 Docker 回环实验环境。"],
  };
  const copy = known[message];
  return copy ? text(...copy) : message;
}

export function connectionFailureMessage(error: unknown, fallback: string): string {
  if (typeof error === "object" && error !== null) {
    const failure = error as { code?: unknown; message_id?: unknown };
    if (typeof failure.message_id === "string" && localizedConnectionCodes.has(failure.message_id)) return failure.message_id;
    if (typeof failure.code === "string" && localizedConnectionCodes.has(failure.code)) return failure.code;
    if (typeof failure.message_id === "string") return failure.message_id;
  }
  return error instanceof Error ? error.message : fallback;
}

export function isConnectionExpiredFailure(error: unknown): boolean {
  return connectionFailureMessage(error, "") === "connection_expired";
}

const localizedConnectionCodes = new Set([
  "connection_expired", "connection_session_expired", "authentication_failed", "permission_denied",
  "tls_failed", "connection_failed", "invalid_request", "tls_required", "capability_unavailable",
  "unsupported_version", "profile_unavailable", "statement_not_allowed", "explain_rejected",
  "remote_timeout", "session_limit",
]);
