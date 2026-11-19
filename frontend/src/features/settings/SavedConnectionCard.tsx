// Task 72 (design C1): the connection saved on the server. Shows where it points and
// whether background collection runs with it; never the password. Forgetting asks twice.
import { useCallback, useEffect, useState } from "react";
import type { JSX } from "react";
import type { SavedConnectionBackground, SavedConnectionView } from "../../../../contracts/generated/doctor-contracts";
import { useLocale } from "../../i18n/locale";
import { doctorApi, type DoctorApi } from "../../lib/api";
import { saveUnavailableText } from "../connection/ConnectionForm";
import { InfoTip } from "../shell/ui";

export function SavedConnectionCard({ api = doctorApi }: { api?: DoctorApi }): JSX.Element {
  const { text } = useLocale();
  const [view, setView] = useState<SavedConnectionView | null>(null);
  const [error, setError] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);

  const load = useCallback((signal?: AbortSignal) => api.savedConnection(signal)
    .then((next) => { if (!signal?.aborted) { setView(next); setError(false); } })
    .catch(() => { if (!signal?.aborted) setError(true); }), [api]);
  useEffect(() => {
    const controller = new AbortController();
    void load(controller.signal);
    // The background state changes on its own (connecting, retrying); a slow poll keeps it current.
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

  const background: Record<SavedConnectionBackground, [string, string]> = {
    off: [text("Not running", "未运行"), "muted"],
    connecting: [text("Connecting…", "正在连接…"), "muted"],
    collecting: [text("Collecting", "采集中"), "ok"],
    standby: [text("Standby: a page session collects; takes over when it ends", "待命：当前由页面会话采集，会话结束后自动接手"), "ok"],
    retrying: [text("Reconnecting with backoff", "连接中断，退避重试中"), "warn"],
    production_gate: [text("Remote cluster: waits for the G1 pilot", "远端集群：等待 G1 试点后才采集"), "warn"],
    unsupported_version: [text("Unsupported StarRocks version", "StarRocks 版本不受支持"), "warn"],
  };
  const hasFields = view?.host && view.port && view.username;
  return <div className="settings-card saved-connection">
    <div className="saved-connection-head">
      <h3>{text("Saved connection", "已保存的连接")} <InfoTip label={text("About the saved connection", "关于已保存的连接")}>{text("The password is sealed with AES-256-GCM in the history store and is write-only. While it is saved, collection keeps running in the background with one bounded connection, and the page connects on open.", "口令用 AES-256-GCM 加密存在历史库，只写不读。保存期间，后台用一条有界连接持续采集，页面打开时自动连接。")}</InfoTip></h3>
      {view?.state === "saved" ? <span className="chip ok">{text("Saved", "已保存")}</span> : null}
      {view?.state === "needs_reentry" ? <span className="chip warn">{text("Needs re-entry", "需重新填写")}</span> : null}
    </div>
    {error ? <p className="saved-connection-note warn" role="alert">{text("Could not read the saved connection.", "无法读取已保存的连接。")}</p> : null}
    {view === null && !error ? <p className="saved-connection-note">{text("Loading…", "正在加载…")}</p> : null}
    {view?.state === "unavailable" && view.unavailable_reason ? <p className="saved-connection-note">{saveUnavailableText(view.unavailable_reason, text)}</p> : null}
    {view?.state === "none" ? <p className="saved-connection-note">{text("Nothing saved. Tick “Save the password encrypted on the SR Doctor server” when connecting.", "未保存。连接时勾选“在 SR Doctor 服务端加密保存口令”即可保存。")}</p> : null}
    {hasFields && view?.state !== "none" ? <dl className="saved-connection-facts">
      <dt>{text("Cluster", "集群")}</dt><dd className="settings-mono">{view.host}:{view.port}</dd>
      <dt>{text("User", "用户名")}</dt><dd className="settings-mono">{view.username}</dd>
      <dt>{text("Background collection", "后台采集")}</dt><dd className={`saved-connection-bg ${background[view.background][1]}`}>{background[view.background][0]}</dd>
    </dl> : null}
    {view?.state === "needs_reentry" ? <p className="saved-connection-note warn">{text("The saved password stopped working, so it is no longer tried. Connect again with the box ticked to replace it.", "已保存的口令失效，不再尝试。重新连接并勾选保存即可替换。")}</p> : null}
    {hasFields ? <div className="saved-connection-actions">
      {confirming ? <>
        <span className="saved-connection-confirm">{text("Forget it and stop background collection?", "忘记它并停止后台采集？")}</span>
        <button className="button secondary" disabled={busy} onClick={() => setConfirming(false)} type="button">{text("Cancel", "取消")}</button>
        <button className="button danger" disabled={busy} onClick={() => void forget()} type="button">{text("Forget", "确认忘记")}</button>
      </> : <button className="button secondary" onClick={() => setConfirming(true)} type="button">{text("Forget saved connection", "忘记已保存的连接")}</button>}
    </div> : null}
  </div>;
}
