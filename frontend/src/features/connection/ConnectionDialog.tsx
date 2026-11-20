import { useCallback, useEffect, useRef } from "react";
import type { Dispatch, JSX } from "react";
import { Dialog, InfoTip } from "../../design";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { doctorApi, type DoctorApi } from "../../lib/api";
import type { ConnectionAction, ConnectionState } from "./connection-state";
import { ConnectionForm } from "./ConnectionForm";

const CONNECTION_ERROR_BY_CODE: Record<string, readonly [string, string]> = {
  connection_expired: ["The local connection session expired. Reconnect to continue.", "本地连接会话已过期，请重新连接。"],
  connection_session_expired: ["The connection expired. Reconnect to continue.", "连接已过期，请重新连接。"],
  authentication_failed: ["StarRocks rejected the supplied credentials.", "用户名或密码未通过验证，请检查后重试。"],
  permission_denied: ["The connection lacks a required read-only permission.", "当前连接缺少必需的只读权限。"],
  tls_failed: ["TLS verify-full failed. Check the server name and certificate chain.", "TLS 完整校验失败，请检查服务器名称与证书链。"],
  connection_failed: ["The StarRocks connection failed.", "连接失败，请检查主机、端口与网络后重试。"],
  invalid_request: ["The connection request is invalid.", "连接信息无效，请检查必填项与端口。"],
  tls_required: ["TLS verify-full is required for non-loopback hosts.", "非回环主机必须使用 TLS 完整校验。"],
  capability_unavailable: ["A required read-only capability is unavailable.", "必需的只读能力不可用。"],
  unsupported_version: [
    "This StarRocks release is not supported: 3.1 and later releases covered by the source audit (3.1–4.1) are.",
    "不支持当前 StarRocks 版本：支持 3.1 及以后、已经过源码审计的版本（3.1～4.1）。",
  ],
  profile_unavailable: ["The requested Profile is unavailable.", "请求的 Profile 不可用。"],
  statement_not_allowed: ["The statement is outside the read-only product boundary.", "该语句超出产品只读边界。"],
  explain_rejected: ["StarRocks rejected the Explain request.", "StarRocks 拒绝了计划检查请求。"],
  remote_timeout: ["The StarRocks request timed out.", "连接请求超时，请检查网络和服务状态。"],
  session_limit: [
    "Too many open connections (4 at most, one of them may be the background collection). Disconnect one you no longer use, or wait a few minutes for idle ones to expire.",
    "同时打开的连接已达上限（最多 4 个，其中可能有 1 个是后台采集）。请断开不用的连接，或等几分钟让空闲连接自动过期。",
  ],
  saved_connection_unavailable: [
    "The password cannot be saved on this server, or the saved one needs re-entry. See Settings › Connection.",
    "当前服务无法保存口令，或已保存的口令需要重新填写，详见 设置 › 连接。",
  ],
  "Connection closed; credentials were removed from the server session.": [
    "Connection closed; credentials were removed from the server session.",
    "连接已关闭，凭据已从服务端会话移除。",
  ],
  "Host, port, and username are required.": [
    "Host, port, and username are required.",
    "主机、端口和用户名均为必填项。",
  ],
  "Host, port, username, and password are required.": [
    "Host, port, username, and password are required.",
    "主机、端口、用户名和密码均为必填项。",
  ],
  "Plaintext is available only for a loopback Docker lab.": [
    "Plaintext is available only for a loopback Docker lab.",
    "明文连接仅允许 Docker 回环实验环境。",
  ],
};

const localizedConnectionCodes = new Set([
  "connection_expired",
  "connection_session_expired",
  "authentication_failed",
  "permission_denied",
  "tls_failed",
  "connection_failed",
  "invalid_request",
  "tls_required",
  "capability_unavailable",
  "unsupported_version",
  "profile_unavailable",
  "statement_not_allowed",
  "explain_rejected",
  "remote_timeout",
  "session_limit",
  "saved_connection_unavailable",
]);

export function ConnectionDialog({
  open,
  onClose,
  onSessionEnded,
  onSessionExpired,
  state,
  dispatch,
  api = doctorApi,
}: {
  open: boolean;
  onClose(): void;
  onSessionEnded?(): void;
  onSessionExpired?(): void;
  state: ConnectionState;
  dispatch: Dispatch<ConnectionAction>;
  api?: DoctorApi;
}): JSX.Element | null {
  const { text } = useLocale();
  const hostInputRef = useRef<HTMLInputElement>(null);
  const dismiss = useCallback(() => onClose(), [onClose]);

  useEffect(() => {
    if (!open) return;
    const frame = typeof window !== "undefined"
      ? window.requestAnimationFrame(() => hostInputRef.current?.focus())
      : null;
    return () => {
      if (frame !== null && typeof window !== "undefined") window.cancelAnimationFrame(frame);
    };
  }, [open]);

  if (!open) return null;

  return (
    <ConnectionForm
      api={api}
      dispatch={dispatch}
      hostInputRef={hostInputRef}
      idPrefix="dialog"
      onDone={dismiss}
      onSessionEnded={onSessionEnded}
      onSessionExpired={onSessionExpired}
      renderLayout={({ body, footer }) => (
        <Dialog
          footer={
            <>
              <span className="mono">{text("Read-only session", "只读会话")}</span>
              <div className="connection-footer-actions">{footer}</div>
            </>
          }
          onOpenChange={(nextOpen) => {
            if (!nextOpen) dismiss();
          }}
          open={open}
          title={
            <span className="card-title-group">
              <span>{text("Connect StarRocks", "连接 StarRocks")}</span>
              <InfoTip label={text("Read-only connection boundary", "只读连接边界说明")}>
                {text(
                  "Read-only MySQL session (multi-statements, DDL, DML, EXPLAIN ANALYZE and Plan Advisor mutations blocked). Runtime Top requires explicit confirmation per run with a 10-second timeout.",
                  "MySQL 协议只读连接（禁止多语句、DDL、DML、EXPLAIN ANALYZE 与 Plan Advisor 修改）；受控执行一次仅接受单条 SELECT/WITH，固定 10 秒超时且每次需二次确认。",
                )}
              </InfoTip>
            </span>
          }
          width="md"
        >
          {body}
        </Dialog>
      )}
      state={state}
    />
  );
}

export function ConnectionStateView({ state }: { state: ConnectionState }): JSX.Element {
  const { text } = useLocale();
  if ((state.phase === "ready" || state.phase === "unsupported") && state.session) {
    return (
      <div
        aria-live="polite"
        className={state.phase === "ready" ? "notice-banner connection-state-banner" : "notice-banner notice-banner--warn connection-state-banner"}
        role="status"
      >
        <div className="connection-state-copy">
          <span className="status-inline">
            <span aria-hidden="true" className={state.phase === "ready" ? "status-dot status-dot--ok" : "status-dot status-dot--warn"} />
            <strong>{state.phase === "ready" ? text("Connected", "已连接") : text("Connected with limited support", "已连接，部分功能受限")}</strong>
          </span>
          <span className="mono">{text("Version", "版本")} {state.session.detected_version?.value ?? text("Unknown", "未知")}</span>
          {state.phase === "unsupported" ? (
            <span>{text("This version is not supported for analysis. Check version compatibility before continuing.", "当前版本暂不支持分析，请先核对版本兼容性。")}</span>
          ) : null}
        </div>
      </div>
    );
  }
  if (state.phase === "connecting") {
    return <div className="notice-banner connection-state-banner" role="status">{text("Connecting…", "连接中…")}</div>;
  }
  if (state.phase === "expired") {
    return (
      <div className="notice-banner notice-banner--warn connection-state-banner" role="alert">
        {state.message === "remote_timeout"
          ? text("A read timed out and the server closed this connection. Reconnect to continue.", "一次读取超时，服务端已关闭此连接，请重新连接。")
          : text("Connection expired. Reconnect to continue.", "连接已过期，请重新连接。")}
      </div>
    );
  }
  if (state.phase === "failed") {
    return (
      <div className="notice-banner notice-banner--danger connection-state-banner" role="alert">
        {connectionFailedCopy(state.message, text)}
      </div>
    );
  }
  if (state.phase === "closed" && state.message) {
    const closedCopy = localizedConnectionMessage(state.message, text);
    if (closedCopy) {
      return <div aria-live="polite" className="notice-banner connection-state-banner" role="status">{closedCopy}</div>;
    }
  }
  return (
    <div aria-live="polite" className="notice-banner connection-state-banner" role="status">
      {text("Not connected. Enter connection details to continue.", "尚未连接，请填写连接信息。")}
    </div>
  );
}

/**
 * Maps known connection error codes/keys directly by code (never by comparing
 * English output against input text), falling back to the generic failure copy.
 */
export function connectionFailedCopy(code: string | null | undefined, text: LocalizedText): string {
  const specific = localizedConnectionMessage(code, text);
  return specific ?? text(
    "Connection failed. Check the connection details and network, then retry.",
    "连接失败，请检查连接信息与网络后重试。",
  );
}

export function localizedConnectionMessage(code: string | null | undefined, text: LocalizedText): string | null {
  if (!code) return null;
  if (Object.prototype.hasOwnProperty.call(CONNECTION_ERROR_BY_CODE, code)) {
    const [en, zh] = CONNECTION_ERROR_BY_CODE[code];
    return text(en, zh);
  }
  return null;
}

export function connectionFailureMessage(error: unknown, fallback: string): string {
  if (typeof error === "object" && error !== null) {
    const failure = error as { code?: unknown; message_id?: unknown };
    if (typeof failure.message_id === "string" && localizedConnectionCodes.has(failure.message_id)) return failure.message_id;
    if (typeof failure.code === "string" && localizedConnectionCodes.has(failure.code)) return failure.code;
    if (typeof failure.code === "string") return failure.code;
    if (typeof failure.message_id === "string") return failure.message_id;
  }
  return error instanceof Error ? error.message : fallback;
}

export function isConnectionExpiredFailure(error: unknown): boolean {
  return connectionFailureMessage(error, "") === "connection_expired";
}
