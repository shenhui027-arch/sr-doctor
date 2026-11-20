// Task 72 / 81 (boards/08-settings.html): current session & server-saved connection.
// Never exposes the password. Forgetting asks twice in the fixed card footer.
import { useCallback, useEffect, useState } from "react";
import type { JSX } from "react";
import type { SavedConnectionBackground, SavedConnectionView, TlsMode } from "../../../../contracts/generated/doctor-contracts";
import { Button, Card, InfoTip, type GridSpan } from "../../design";
import { lookupEnumLabel } from "../../i18n/enum-label";
import { useLocale } from "../../i18n/locale";
import { doctorApi, type DoctorApi } from "../../lib/api";
import { saveUnavailableText } from "../connection/ConnectionForm";

const BACKGROUND_LABELS: Record<SavedConnectionBackground, readonly [string, string]> = {
  off: ["Not running", "未运行"],
  connecting: ["Connecting…", "正在连接…"],
  collecting: ["Collecting", "采集中"],
  standby: ["Standby (page session collects; takes over when closed)", "待命（当前由页面会话采集，关闭网页后自动接手）"],
  retrying: ["Reconnecting with backoff", "连接中断，退避重试中"],
  production_gate: ["Remote cluster: waits for the G1 pilot", "远端集群：等待 G1 试点后才采集"],
  unsupported_version: ["Unsupported StarRocks version", "StarRocks 版本不受支持"],
};

function renderBackgroundStatus(bg: SavedConnectionBackground, label: string): JSX.Element {
  switch (bg) {
    case "collecting":
    case "standby":
      return (
        <span className="status-inline">
          <span aria-hidden="true" className="status-dot status-dot--ok" />
          {label}
        </span>
      );
    case "retrying":
    case "production_gate":
    case "unsupported_version":
      return <span className="state-stamp state-stamp--warn">{label}</span>;
    default:
      return <span className="state-stamp">{label}</span>;
  }
}

export function SavedConnectionCard({
  api = doctorApi,
  connectionStatus,
  tlsMode = null,
  onOpenConnection,
  span,
}: {
  api?: DoctorApi;
  connectionStatus?: string;
  tlsMode?: TlsMode | null;
  onOpenConnection?(): void;
  span?: GridSpan;
}): JSX.Element {
  const { text } = useLocale();
  const [view, setView] = useState<SavedConnectionView | null>(null);
  const [error, setError] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);

  const load = useCallback(
    (signal?: AbortSignal) =>
      api.savedConnection(signal)
        .then((next) => { if (!signal?.aborted) { setView(next); setError(false); } })
        .catch(() => { if (!signal?.aborted) setError(true); }),
    [api],
  );

  useEffect(() => {
    const controller = new AbortController();
    void load(controller.signal);
    const timer = window.setInterval(() => void load(controller.signal), 15_000);
    return () => { controller.abort(); window.clearInterval(timer); };
  }, [load]);

  async function forget() {
    setBusy(true);
    try {
      await api.forgetSavedConnection();
      setConfirming(false);
      await load();
    } catch {
      setError(true);
    } finally {
      setBusy(false);
    }
  }

  const hasFields = Boolean(view?.host && view.port && view.username);
  const isPlaintextSession = tlsMode === "plaintext" || tlsMode === "docker_loopback_plaintext";

  const statusStamp = view?.state === "saved" ? (
    <span className="status-inline">
      <span aria-hidden="true" className="status-dot status-dot--ok" />
      {text("Saved", "已保存")}
    </span>
  ) : view?.state === "needs_reentry" ? (
    <span className="state-stamp state-stamp--warn">{text("Needs re-entry", "需重新填写")}</span>
  ) : null;

  return (
    <Card
      actions={statusStamp}
      footer={
        hasFields ? (
          confirming ? (
            <div className="saved-conn-footer-bar">
              <span className="saved-conn-confirm-text">{text("Forget it and stop background collection?", "忘记口令并停止无页面后台采集？")}</span>
              <div className="saved-conn-footer-btns">
                <Button disabled={busy} onClick={() => setConfirming(false)} size="sm" variant="secondary">
                  {text("Cancel", "取消")}
                </Button>
                <Button disabled={busy} onClick={() => void forget()} size="sm" variant="danger">
                  {text("Forget", "确认忘记")}
                </Button>
              </div>
            </div>
          ) : (
            <div className="saved-conn-footer-bar">
              <span>{text("At most 1 encrypted connection saved", "全站最多保存 1 个加密连接")}</span>
              <Button onClick={() => setConfirming(true)} size="sm" variant="danger-outline">
                {text("Forget saved connection…", "忘记已保存的连接…")}
              </Button>
            </div>
          )
        ) : (
          <span>{text("At most 1 encrypted connection saved", "全站最多保存 1 个加密连接")}</span>
        )
      }
      span={span}
      title={text("Saved connection", "当前会话与加密保存的连接")}
      titleExtra={
        <InfoTip label={text("About the saved connection", "关于已保存的连接")}>
          {text(
            "The password is sealed with AES-256-GCM in the history store and is write-only. While saved, collection keeps running in the background with one bounded connection, and the page connects on open.",
            "口令用 AES-256-GCM 加密存入本机历史库，只写不读。保存期间，后台用一条有界连接持续采集，页面打开时自动连接。",
          )}
        </InfoTip>
      }
      variant="detail"
    >
      <div className="saved-connection-stack">
        {connectionStatus && onOpenConnection ? (
          <div className="current-session-box">
            <div className="current-session-info">
              <span className="field-label">{text("Current page session", "当前页面会话")}</span>
              <span className="mono">{connectionStatus}</span>
              {tlsMode ? (
                <span className="badge-stamp">
                  {tlsMode === "verify_full" ? "TLS verify-full" : text("Plaintext", "明文连接")}
                </span>
              ) : null}
            </div>
            <Button onClick={onOpenConnection} size="sm" variant="secondary">
              {text("Manage connection…", "管理连接…")}
            </Button>
          </div>
        ) : null}

        {isPlaintextSession ? (
          <div className="notice-banner notice-banner--warn" role="note">
            <span>
              <strong>{text("Plaintext transport:", "明文传输提醒：")}</strong>{" "}
              {text(
                "Current session sends password and SQL unencrypted; enable FE SSL and choose TLS verify-full on remote clusters.",
                "当前会话使用明文连接，口令与 SQL 不加密经过网络；远端集群建议开启 FE SSL 并选用 TLS verify-full。",
              )}
            </span>
          </div>
        ) : null}

        {error ? (
          <div className="notice-banner notice-banner--warn" role="alert">
            <span>{text("Could not read the saved connection.", "无法读取已保存的连接。")}</span>
          </div>
        ) : null}

        {view === null && !error ? (
          <p className="card-subtitle">{text("Loading…", "正在加载…")}</p>
        ) : null}

        {view?.state === "unavailable" && view.unavailable_reason ? (
          <div
            className={view.unavailable_reason === "default_admin_password" ? "notice-banner notice-banner--danger" : "notice-banner notice-banner--warn"}
            role="alert"
          >
            <span>{saveUnavailableText(view.unavailable_reason, text)}</span>
          </div>
        ) : null}

        {view?.state === "none" ? (
          <p className="card-subtitle">
            {text(
              "Nothing saved. Tick “Save the password encrypted on the SR Doctor server” when connecting.",
              "未保存。连接时勾选“在服务端加密保存口令，持续后台采集”即可保存。",
            )}
          </p>
        ) : null}

        {view?.state === "needs_reentry" ? (
          <div className="notice-banner notice-banner--warn" role="alert">
            <span>
              <strong>{text("Re-enter password:", "需重新填写口令：")}</strong>{" "}
              {text(
                "The saved password stopped working or cannot be decrypted, so automatic retries stopped. Reconnect with the save checkbox ticked to replace it.",
                "已保存的口令认证失败或密文无法解密，服务端已停止自动重试。请重新连接并勾选保存以替换。",
              )}
            </span>
          </div>
        ) : null}

        {hasFields && view && view.state !== "none" ? (
          <>
            <dl className="kv-grid">
              <dt>{text("Cluster & user", "集群与账号")}</dt>
              <dd className="mono">{view.host}:{view.port} · {view.username}</dd>
              <dt>{text("Protection", "凭据保护")}</dt>
              <dd><code>AES-256-GCM</code> {text("write-only in local history DB (never returned, logged or exported)", "存入本机历史库，只写不读（永不返回前端、不写日志、不进导出）")}</dd>
              <dt>{text("Background collection", "后台持续采集")}</dt>
              <dd>{renderBackgroundStatus(view.background, lookupEnumLabel(BACKGROUND_LABELS, view.background, text))}</dd>
            </dl>

            <div className="notice-banner" role="note">
              <span>
                <strong>{text("Forgetting deletes:", "忘记已保存连接的后果：")}</strong>{" "}
                {text(
                  "Immediately deletes the encrypted password on the server and stops background collection once all open browser pages close (existing history and governance data stay).",
                  "将立即删除服务端加密保存的 StarRocks 口令，并在所有浏览器页面关闭后立即停止无页面时的后台持续采集（历史库中已有趋势与治理数据保留）。",
                )}
              </span>
            </div>
          </>
        ) : null}
      </div>
    </Card>
  );
}
