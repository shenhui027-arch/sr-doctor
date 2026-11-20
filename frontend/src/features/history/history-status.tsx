// What the app shell needs from the monitoring workspaces on every page: the
// history status poll, the header collection capsule and the tab type. Kept
// apart so the trends, alerts and governance pages can load on demand.
import { useCallback, useEffect, useState } from "react";
import type { JSX } from "react";
import type { CollectionStoppedReason, HistoryStatusResponse } from "../../../../contracts/generated/doctor-contracts";
import { lookupEnumLabel } from "../../i18n/enum-label";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { doctorApi } from "../../lib/api";
import { HeaderStatusText } from "../shell/HeaderStatusText";
import { failureId } from "./monitor-read";
import "./history-status.css";

export function clockText(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined) return "—";
  return new Date(seconds * 1000).toISOString().slice(0, 19).replace("T", " ");
}

const STOPPED_REASON_LABELS: Record<CollectionStoppedReason, readonly [string, string]> = {
  not_connected: ["Not collecting · reconnect to continue", "未采集 · 重新连接后继续"],
  history_disabled: ["History off", "历史已关闭"],
  production_gate: ["Remote collection not enabled yet", "远端采集尚未启用"],
  audit_unavailable: ["Not collecting · no audit table", "未采集 · 无审计表"],
  another_session_collecting: ["Not collecting · another session collects", "未采集 · 另一会话正在采集"],
};

export function collectionLabel(status: HistoryStatusResponse, text: LocalizedText): string {
  const state = status.collection;
  if (!state || typeof state !== "object") return lookupEnumLabel(STOPPED_REASON_LABELS, "", text);
  if (state.state === "running") return text("Collecting", "采集中");
  if (state.state === "paused") return text("Paused", "已暂停");
  if (state.state === "backoff") return text("Retrying shortly", "稍后自动重试");
  if (state.state === "stopped") {
    return lookupEnumLabel(STOPPED_REASON_LABELS, state.reason, text);
  }
  return lookupEnumLabel(STOPPED_REASON_LABELS, String((state as { state?: unknown }).state ?? ""), text);
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
  const state = status.collection?.state ?? "stopped";
  const label = collectionLabel(status, text);
  const dotClass = state === "running"
    ? "status-dot status-dot--ok"
    : state === "backoff"
      ? "status-dot status-dot--warn"
      : "status-dot";
  const watermark = status.watermark_seconds !== null
    ? ` · ${text("watermark", "水位")} ${clockText(status.watermark_seconds).slice(11, 16)}`
    : "";
  return (
    <button
      className="collector-status hide-lt-1024"
      onClick={onOpen}
      title={`${text("Collection and history", "采集与历史")} · ${label}${watermark}`}
      type="button"
    >
      <span aria-hidden="true" className={dotClass} />
      <span className="hide-lt-1440">{text("Collector · ", "后台采集 · ")}</span>
      <span><HeaderStatusText value={label} /></span>
    </button>
  );
}

export type MonitoringTab = "trends" | "collection";
