// Shared pieces of the monitoring views: which target they show, when the
// data was read, and what to do when history is off or a read fails.
import type { JSX } from "react";
import type { HistoryStoreState, HistoryStoreUnavailableReason } from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { clockText } from "./CollectionPanel";

/** The target a view shows; a picker only when the store knows several. */
export function TargetPicker({ target, targets, onTarget, text }: { target: string | null; targets: string[]; onTarget?(target: string): void; text: LocalizedText }): JSX.Element | null {
  if (!target) return null;
  if (!onTarget || targets.length < 2) return <span className="monitor-target">{text("Cluster", "集群")}<strong>{target}</strong></span>;
  return <label className="monitor-target">{text("Cluster", "集群")}<select onChange={(event) => onTarget(event.target.value)} value={target}>
    {targets.map((item) => <option key={item} value={item}>{item}</option>)}
  </select></label>;
}

/** When the data on screen was read, whether the last refresh failed, and a manual refresh. */
export function RefreshBar({ updatedSeconds, error, loading, onRefresh, text }: { updatedSeconds: number | null; error: string | null; loading: boolean; onRefresh(): void; text: LocalizedText }): JSX.Element {
  const at = updatedSeconds === null ? null : `${clockText(updatedSeconds).slice(11, 19)} UTC`;
  return <span className="monitor-refresh">
    {error && at ? <span className="stale" role="alert">{text(`Refresh failed (${error}); showing data from ${at}`, `刷新失败（${error}），显示的是 ${at} 的结果`)}</span>
      : at ? <span role="status">{text(`Updated ${at}`, `更新于 ${at}`)}</span> : null}
    <button className="button secondary" disabled={loading} onClick={onRefresh} type="button">{loading ? text("Refreshing…", "正在刷新…") : text("Refresh", "刷新")}</button>
  </span>;
}

/** What to do about an unopenable store, by the server's reason category. */
function unavailableAdvice(reason: HistoryStoreUnavailableReason | undefined, text: LocalizedText): string {
  switch (reason) {
    case "migration_requires_manual_resolution": return text(
      "Upgrading the store found data that needs your decision: one template has two open governance items. History was left unchanged. In the previous version, mark the extra item \"Won't fix\" and restart, or ask for a dedicated data repair.",
      "升级历史库时发现需要你决定的数据：同一模板有两个进行中的治理项。原有历史未被改动。请在旧版本中把多余的一项标为“不处理”后重启，或另行安排专项数据修复。",
    );
    case "io_or_permission": return text(
      "Check that the folder of DOCTOR_HISTORY_PATH exists and is writable, restart doctor-api, then check again.",
      "请确认 DOCTOR_HISTORY_PATH 所在目录存在且可写，重启 doctor-api 后再检查。",
    );
    default: return text(
      "Check that the folder of DOCTOR_HISTORY_PATH exists and is writable, restart doctor-api, then check again; the doctor-api console prints the error code.",
      "请确认 DOCTOR_HISTORY_PATH 所在目录存在且可写，重启 doctor-api 后再检查；doctor-api 控制台会输出错误码。",
    );
  }
}

/** History is off or could not be opened: say why and what still works. */
export function HistoryOffState({ store, detail, onRetry, onOpenQueries, text }: {
  store: HistoryStoreState["state"];
  /** The status poll's store, when known: carries the unavailable reason and code. */
  detail?: HistoryStoreState;
  onRetry(): void;
  onOpenQueries?(): void;
  text: LocalizedText;
}): JSX.Element {
  const unavailable = detail?.state === "unavailable" ? detail : null;
  return <section aria-labelledby="history-off-title" className="history-card monitor-empty">
    <h2 id="history-off-title">{store === "disabled" ? text("History monitoring is off in this setup", "此运行方式未开启历史监控") : text("The local history store could not be opened", "无法打开本机历史库")}</h2>
    {store === "disabled" ? <>
      <p>{text("Trends, alerts and the governance queue need the local history store. This setup runs with DOCTOR_HISTORY=off, as the single-machine container (deploy/local/compose.yaml) does.", "趋势、告警与治理队列都需要本机历史库。当前运行方式设置了 DOCTOR_HISTORY=off（deploy/local/compose.yaml 的单机容器就是这样）。")}</p>
      <p>{text("To use them, deploy with ./deploy.sh, or run doctor-api as a local process without DOCTOR_HISTORY=off; see docs/guide/deployment.md.", "需要时改用 ./deploy.sh 部署，或以本机进程方式启动 doctor-api 且不设置 DOCTOR_HISTORY=off，详见 docs/guide/deployment.md。")}</p>
    </> : <>
      <p>{unavailableAdvice(unavailable?.reason, text)}</p>
      {unavailable?.code && <p className="history-muted">{text("Error code", "错误码")} <code>{unavailable.code}</code></p>}
    </>}
    <p className="history-muted">{text("Profile import, diagnosis and Compare work without it.", "Profile 导入、诊断与对比不受影响。")}</p>
    <div className="alert-actions">
      <button className="button secondary" onClick={onRetry} type="button">{text("Check again", "重新检查")}</button>
      {onOpenQueries && <button className="button secondary" onClick={onOpenQueries} type="button">{text("Go to queries", "去查询与诊断")}</button>}
    </div>
  </section>;
}

/** A first read failed and there is nothing to show yet. */
export function MonitorReadError({ title, error, onRetry, text }: { title: string; error: string; onRetry(): void; text: LocalizedText }): JSX.Element {
  return <div className="capability-error monitor-read-error" role="alert">
    <span>{title} <span className="history-muted">({error})</span></span>
    <button className="button secondary" onClick={onRetry} type="button">{text("Try again", "重试")}</button>
  </div>;
}

/** The store state a monitoring view must stop at, or null when it may read. */
export function blockedStore(store: HistoryStoreState | undefined, error: string | null): HistoryStoreState["state"] | null {
  if (error === "history_disabled") return "disabled";
  if (store && store.state !== "available") return store.state;
  return null;
}
