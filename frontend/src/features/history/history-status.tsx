// What the app shell needs from the monitoring workspaces on every page: the
// history status poll, the header collection capsule and the tab type. Kept
// apart so the trends, alerts and governance pages can load on demand.
import { useCallback, useEffect, useState } from "react";
import type { JSX } from "react";
import type { HistoryStatusResponse } from "../../../../contracts/generated/doctor-contracts";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { doctorApi } from "../../lib/api";
import { HeaderStatusText } from "../shell/HeaderStatusText";
import { failureId } from "./monitor-read";
import "./history-status.css";

export function clockText(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined) return "—";
  return new Date(seconds * 1000).toISOString().slice(0, 19).replace("T", " ");
}

export function collectionLabel(status: HistoryStatusResponse, text: LocalizedText): string {
  const state = status.collection;
  if (state.state === "running") return text("Collecting", "采集中");
  if (state.state === "paused") return text("Paused", "已暂停");
  if (state.state === "backoff") return text("Retrying shortly", "稍后自动重试");
  return {
    not_connected: text("Not collecting · reconnect to continue", "未采集 · 重新连接后继续"),
    history_disabled: text("History off", "历史已关闭"),
    production_gate: text("Remote collection not enabled yet", "远端采集尚未启用"),
    audit_unavailable: text("Not collecting · no audit table", "未采集 · 无审计表"),
    another_session_collecting: text("Not collecting · another session collects", "未采集 · 另一会话正在采集"),
  }[state.reason];
}

export const HISTORY_POLL_MS = 15_000;

export type HistoryStatusState = { status: HistoryStatusResponse | null; error: string | null; refresh(): void };

/** One bounded poll of the local API; it never talks to StarRocks itself. */
export function useHistoryStatus(connectionId: string | null): HistoryStatusState {
  const [status, setStatus] = useState<HistoryStatusResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tick, setTick] = useState(0);
  const refresh = useCallback(() => setTick((value) => value + 1), []);
  useEffect(() => {
    const controller = new AbortController();
    doctorApi.readHistoryStatus(connectionId, controller.signal)
      .then((value) => { if (!controller.signal.aborted) { setStatus(value); setError(null); } })
      .catch((cause: unknown) => { if (!controller.signal.aborted) setError(failureId(cause, "history_status_failed")); });
    const timer = window.setTimeout(refresh, HISTORY_POLL_MS);
    return () => { controller.abort(); window.clearTimeout(timer); };
  }, [connectionId, refresh, tick]);
  return { status, error, refresh };
}

/**
 * Changes whenever the collector stores a slice or the alert count moves, so
 * the trends and alert views re-read exactly when there is something new.
 */
export function monitorGeneration(status: HistoryStatusResponse | null): string {
  return status ? `${status.watermark_seconds ?? "-"}|${status.open_alerts}|${status.load_revision ?? "-"}` : "";
}

export function CollectionCapsule({ status, onOpen }: { status: HistoryStatusResponse | null; onOpen(): void }): JSX.Element | null {
  const { text } = useLocale();
  if (!status || status.store.state !== "available") return null;
  const state = status.collection.state;
  return <button className={`collection-capsule ${state}`} onClick={onOpen} title={`${text("Collection and history", "采集与历史")} · ${collectionLabel(status, text)}`} type="button">
    <HeaderStatusText value={collectionLabel(status, text)} />
    {status.watermark_seconds !== null && <span className="collection-watermark header-detail">{text("watermark", "水位")} {clockText(status.watermark_seconds).slice(11, 16)}</span>}
  </button>;
}

export type MonitoringTab = "trends" | "collection";
