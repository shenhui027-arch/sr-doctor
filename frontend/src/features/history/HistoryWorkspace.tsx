// Direct trend loader (boards/02-overview.html): mounts `TrendsPanel` directly
// with NO duplicate `.history-tabs` wrapper, and guards every fetch with an
// `AbortController` so superseded or unmounted requests never overwrite the view.
import { useEffect, useRef, useState } from "react";
import type { JSX } from "react";
import type {
  HistorySeriesRequest,
  HistorySeriesResponse,
  HistoryTrendsRequest,
  HistoryTrendsResponse,
  ProfileFetchResponse,
  TrendExemplar,
  TrendRow,
} from "../../../../contracts/generated/doctor-contracts";
import { useLocale } from "../../i18n/locale";
import { doctorApi } from "../../lib/api";
import { CollectionPanel } from "./CollectionPanel";
import { monitorGeneration, type HistoryStatusState, type MonitoringTab } from "./history-status";
import { failureId, useMonitorRead } from "./monitor-read";
import { governanceActionText } from "./MonitoringWorkspaces";
import { blockedStore, HistoryOffState, MonitorReadError, RefreshBar, TargetPicker } from "./MonitorStates";
import { TrendsPanel } from "./TrendsPanel";
import "./history.css";

export { CollectionCapsule, HISTORY_POLL_MS, monitorGeneration, useHistoryStatus, type HistoryStatusState, type MonitoringTab } from "./history-status";

export type TrendFetchApi = {
  readHistoryTrends(request: HistoryTrendsRequest, signal?: AbortSignal): Promise<HistoryTrendsResponse>;
  readHistorySeries(request: HistorySeriesRequest, signal?: AbortSignal): Promise<HistorySeriesResponse>;
};

export type TrendRequestTracker = {
  fetchTrends(request: HistoryTrendsRequest, onAccept: (data: HistoryTrendsResponse) => void, onError: (code: string) => void): Promise<void>;
  fetchSeries(request: HistorySeriesRequest, onAccept: (data: HistorySeriesResponse) => void, onError: (code: string) => void): Promise<void>;
  abortAll(): void;
};

/**
 * Coordinates `AbortController` lifecycles for trend and series fetches:
 * starting a new request of the same kind aborts the superseded one, and
 * `abortAll()` aborts every in-flight request on unmount so late responses
 * never overwrite the current view.
 */
export function createTrendRequestTracker(api: TrendFetchApi): TrendRequestTracker {
  let trendsCtrl: AbortController | null = null;
  let seriesCtrl: AbortController | null = null;

  return {
    async fetchTrends(request, onAccept, onError) {
      trendsCtrl?.abort();
      const controller = new AbortController();
      trendsCtrl = controller;
      try {
        const value = await api.readHistoryTrends(request, controller.signal);
        if (trendsCtrl === controller && !controller.signal.aborted) onAccept(value);
      } catch (cause) {
        if (trendsCtrl === controller && !controller.signal.aborted) onError(failureId(cause, "history_trends_failed"));
      } finally {
        if (trendsCtrl === controller) trendsCtrl = null;
      }
    },
    async fetchSeries(request, onAccept, onError) {
      seriesCtrl?.abort();
      const controller = new AbortController();
      seriesCtrl = controller;
      try {
        const value = await api.readHistorySeries(request, controller.signal);
        if (seriesCtrl === controller && !controller.signal.aborted) onAccept(value);
      } catch (cause) {
        if (seriesCtrl === controller && !controller.signal.aborted) onError(failureId(cause, "history_series_failed"));
      } finally {
        if (seriesCtrl === controller) seriesCtrl = null;
      }
    },
    abortAll() {
      trendsCtrl?.abort();
      trendsCtrl = null;
      seriesCtrl?.abort();
      seriesCtrl = null;
    },
  };
}

/**
 * Direct 30-day template trends workspace for Overview (no duplicate second-level tab).
 */
export function TrendsWorkspace({
  connectionId,
  target = null,
  onTarget = () => undefined,
  onTargets,
  focusTemplate = null,
  history,
  onOpenDoctor,
  onOpenItem,
  onOpenQueries,
  onSessionExpired,
}: {
  connectionId: string | null;
  target?: string | null;
  onTarget?(target: string): void;
  onTargets?(targets: string[]): void;
  focusTemplate?: string | null;
  onOpenItem?(itemId: string): void;
  onOpenQueries?(): void;
  history: HistoryStatusState;
  onOpenDoctor?(profile: ProfileFetchResponse): void;
  onSessionExpired?(): void;
}): JSX.Element {
  const { text } = useLocale();
  const blocked = blockedStore(history.status?.store, null);
  const generation = monitorGeneration(history.status);
  const [windows, setWindows] = useState({ baseline: 168, current: 24 });
  const [selected, setSelected] = useState<string | null>(focusTemplate);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pulling, setPulling] = useState<string | null>(null);
  const actionControllerRef = useRef<AbortController | null>(null);

  useEffect(() => {
    if (focusTemplate) setSelected(focusTemplate);
  }, [focusTemplate]);

  useEffect(() => () => {
    actionControllerRef.current?.abort();
    actionControllerRef.current = null;
  }, []);

  const trends = useMonitorRead(
    blocked
      ? null
      : (signal) => doctorApi.readHistoryTrends({ target, current_hours: windows.current, baseline_hours: windows.baseline, limit: 100 }, signal),
    `${target ?? ""}|${windows.baseline}|${windows.current}`,
    generation,
    "history_trends_failed",
  );

  const shown = trends.data?.target ?? null;

  useEffect(() => {
    if (trends.data) onTargets?.(trends.data.targets);
  }, [trends.data, onTargets]);

  useEffect(() => {
    const value = trends.data;
    if (!value) return;
    setSelected((key) =>
      key !== null && (key === focusTemplate || value.rows.some((row) => row.template_key === key))
        ? key
        : value.rows[0]?.template_key ?? null,
    );
  }, [trends.data, focusTemplate]);

  const series = useMonitorRead(
    !blocked && shown && selected
      ? (signal) =>
          doctorApi.readHistorySeries(
            { target: shown, template_key: selected, current_hours: windows.current, baseline_hours: windows.baseline, points: 48 },
            signal,
          )
      : null,
    `${shown ?? ""}|${selected ?? ""}|${windows.baseline}|${windows.current}`,
    generation,
    "history_series_failed",
  );

  if (blocked) {
    return <HistoryOffState detail={history.status?.store} onOpenQueries={onOpenQueries} onRetry={history.refresh} store={blocked} text={text} />;
  }

  const sameTarget = connectionId !== null && history.status?.target === shown && history.status.collection.state !== "stopped";

  const pull = async (exemplar: TrendExemplar, row: TrendRow) => {
    if (!connectionId || !onOpenDoctor) return;
    actionControllerRef.current?.abort();
    const controller = new AbortController();
    actionControllerRef.current = controller;
    setPulling(exemplar.query_id);
    setActionError(null);
    try {
      const occurrences = await doctorApi.readAuditOccurrences(
        {
          connection_id: connectionId,
          request: {
            scope: {
              window: { start_seconds: exemplar.event_seconds, end_seconds: exemplar.event_seconds + 1 },
              state: null,
              user: null,
              native_digest: null,
              catalog: row.catalog,
              database: row.database,
              resource_group: null,
              statement_filter: "exclude_doctor_and_non_query",
            },
            sort: "query_time_desc",
            limit: 50,
          },
        },
        controller.signal,
      );
      if (controller.signal.aborted || actionControllerRef.current !== controller) return;
      if (!occurrences.rows.some((item) => item.query.query_id === exemplar.query_id)) {
        setActionError("exemplar_not_found_in_audit");
        return;
      }
      const profile = await doctorApi.fetchConnectedProfile(
        {
          connection_id: connectionId,
          query_id: exemplar.query_id,
          discovery_generation: occurrences.discovery_generation,
          kind: "baseline",
        },
        controller.signal,
      );
      if (controller.signal.aborted || actionControllerRef.current !== controller) return;
      onOpenDoctor(profile);
    } catch (cause) {
      if (controller.signal.aborted || actionControllerRef.current !== controller) return;
      const id = failureId(cause, "profile_fetch_failed");
      if (id === "connection_expired" || id === "connection_session_expired") onSessionExpired?.();
      setActionError(id);
    } finally {
      if (actionControllerRef.current === controller) {
        actionControllerRef.current = null;
        setPulling(null);
      }
    }
  };

  const refresh = () => {
    trends.refresh();
    series.refresh();
  };

  if (!trends.data) {
    if (trends.error === "history_disabled") {
      return <HistoryOffState onOpenQueries={onOpenQueries} onRetry={() => { history.refresh(); refresh(); }} store="disabled" text={text} />;
    }
    return trends.error ? (
      <MonitorReadError error={trends.error} onRetry={refresh} text={text} title={text("Trends could not be read.", "趋势暂时无法读取。")} />
    ) : (
      <p className="workspace-loading" role="status">{text("Loading…", "正在加载…")}</p>
    );
  }

  return (
    <>
      {actionError ? (
        <div className="notice-banner notice-banner--danger" role="alert">
          <span>
            {actionError === "exemplar_not_found_in_audit"
              ? text("This execution is no longer in the audit table window; its Profile cannot be pulled.", "审计表中已找不到这次执行，无法拉取 Profile。")
              : governanceActionText(actionError, text)}
          </span>
        </div>
      ) : null}
      {series.error && !series.data && selected ? (
        <MonitorReadError error={series.error} onRetry={series.refresh} text={text} title={text("This template's trend could not be read.", "该模板的趋势暂时无法读取。")} />
      ) : null}
      {pulling ? <p className="workspace-loading" role="status">{text("Pulling Profile…", "正在拉取 Profile…")}</p> : null}
      <TrendsPanel
        baselineHours={windows.baseline}
        currentHours={windows.current}
        onAddToQueue={
          onOpenItem
            ? (row) => {
                if (!shown) return;
                actionControllerRef.current?.abort();
                const controller = new AbortController();
                actionControllerRef.current = controller;
                setActionError(null);
                doctorApi.createGovernanceItem({ target: shown, template_key: row.template_key, origin: "trends", alert_id: null, title: null }, controller.signal)
                  .then((item) => {
                    if (!controller.signal.aborted && actionControllerRef.current === controller) onOpenItem(item.item_id);
                  })
                  .catch((cause: unknown) => {
                    if (!controller.signal.aborted && actionControllerRef.current === controller) setActionError(failureId(cause, "request_failed"));
                  })
                  .finally(() => {
                    if (actionControllerRef.current === controller) actionControllerRef.current = null;
                  });
              }
            : undefined
        }
        onPullProfile={
          sameTarget && onOpenDoctor && series.data
            ? (queryId) => {
                const exemplar = series.data?.exemplars.find((item) => item.query_id === queryId);
                if (exemplar && series.data && !pulling) void pull(exemplar, series.data.row);
              }
            : undefined
        }
        onSelect={setSelected}
        onWindow={(baseline, current) => setWindows({ baseline, current })}
        selectedKey={selected}
        series={series.data}
        text={text}
        toolbar={
          <span className="trend-toolbar">
            <TargetPicker onTarget={onTarget} target={shown} targets={trends.data.targets} text={text} />
            <RefreshBar error={trends.error ?? series.error} loading={trends.loading || series.loading} onRefresh={refresh} text={text} updatedSeconds={trends.updatedSeconds} />
          </span>
        }
        trends={trends.data}
      />
    </>
  );
}

/** Thin direct delegate without the legacy `.history-tabs` wrapper. */
export function HistoryWorkspace(props: {
  connectionId: string | null;
  target?: string | null;
  onTarget?(target: string): void;
  onTargets?(targets: string[]): void;
  focusTemplate?: string | null;
  onOpenItem?(itemId: string): void;
  onOpenQueries?(): void;
  tab: MonitoringTab;
  onTab?(tab: MonitoringTab): void;
  history: HistoryStatusState;
  onOpenDoctor?(profile: ProfileFetchResponse): void;
  onSessionExpired?(): void;
}): JSX.Element {
  const { text } = useLocale();
  const [busy, setBusy] = useState(false);
  const { connectionId, history, tab } = props;

  if (tab === "trends") {
    return <TrendsWorkspace {...props} />;
  }

  const run = (work: () => Promise<void>) => {
    setBusy(true);
    work()
      .catch(() => undefined)
      .finally(() => {
        setBusy(false);
        history.refresh();
      });
  };

  if (!history.status) {
    return history.error ? (
      <MonitorReadError error={history.error} onRetry={history.refresh} text={text} title={text("History status could not be read.", "历史状态暂时无法读取。")} />
    ) : (
      <p className="workspace-loading" role="status">{text("Loading…", "正在加载…")}</p>
    );
  }

  return (
    <CollectionPanel
      busy={busy}
      onClear={() => run(() => doctorApi.clearHistory())}
      onToggle={(action) => run(() => doctorApi.setHistoryCollection({ connection_id: connectionId, action }))}
      status={history.status}
      text={text}
    />
  );
}
