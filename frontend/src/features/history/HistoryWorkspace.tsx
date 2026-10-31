// The monitoring workspace (design boards B-D). It owns one status poll for
// the header capsule, the trends tab and the collection tab; the server
// decides every state and every statistic.
import { useEffect, useState } from "react";
import type { JSX } from "react";
import type { ProfileFetchResponse, TrendExemplar, TrendRow } from "../../../../contracts/generated/doctor-contracts";
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

/** Board C: trends and one template's series, re-read when the window, target, selection or collection changes. */
function TrendsTab({ connectionId, target, onTarget, onTargets, generation, focusTemplate, history, onOpenDoctor, onOpenItem, onOpenQueries, onSessionExpired }: {
  connectionId: string | null;
  target: string | null;
  onTarget(target: string): void;
  onTargets?(targets: string[]): void;
  generation: string;
  focusTemplate: string | null;
  onOpenItem?(itemId: string): void;
  onOpenQueries?(): void;
  history: HistoryStatusState;
  onOpenDoctor?(profile: ProfileFetchResponse): void;
  onSessionExpired?(): void;
}): JSX.Element {
  const { text } = useLocale();
  const [windows, setWindows] = useState({ baseline: 168, current: 24 });
  const [selected, setSelected] = useState<string | null>(focusTemplate);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pulling, setPulling] = useState<string | null>(null);
  useEffect(() => { if (focusTemplate) setSelected(focusTemplate); }, [focusTemplate]);
  const trends = useMonitorRead(
    (signal) => doctorApi.readHistoryTrends({ target, current_hours: windows.current, baseline_hours: windows.baseline, limit: 100 }, signal),
    `${target ?? ""}|${windows.baseline}|${windows.current}`, generation, "history_trends_failed",
  );
  const shown = trends.data?.target ?? null;
  useEffect(() => { if (trends.data) onTargets?.(trends.data.targets); }, [trends.data, onTargets]);
  useEffect(() => {
    const value = trends.data;
    if (!value) return;
    // A template opened from an alert or a governance item stays selected even when it is not a listed verdict.
    setSelected((key) => key !== null && (key === focusTemplate || value.rows.some((row) => row.template_key === key)) ? key : value.rows[0]?.template_key ?? null);
  }, [trends.data, focusTemplate]);
  const series = useMonitorRead(
    shown && selected ? (signal) => doctorApi.readHistorySeries({ target: shown, template_key: selected, current_hours: windows.current, baseline_hours: windows.baseline, points: 48 }, signal) : null,
    `${shown ?? ""}|${selected ?? ""}|${windows.baseline}|${windows.current}`, generation, "history_series_failed",
  );

  // A Profile is fetched only for a query this session has discovered: one
  // bounded audit read of the exemplar's second, then the ordinary fetch.
  const sameTarget = connectionId !== null && history.status?.target === shown && history.status.collection.state !== "stopped";
  const pull = async (exemplar: TrendExemplar, row: TrendRow) => {
    if (!connectionId || !onOpenDoctor) return;
    setPulling(exemplar.query_id);
    setActionError(null);
    try {
      const occurrences = await doctorApi.readAuditOccurrences({ connection_id: connectionId, request: {
        scope: { window: { start_seconds: exemplar.event_seconds, end_seconds: exemplar.event_seconds + 1 }, state: null, user: null, native_digest: null, catalog: row.catalog, database: row.database, resource_group: null, statement_filter: "exclude_doctor_and_non_query" },
        sort: "query_time_desc",
        limit: 50,
      } });
      if (!occurrences.rows.some((item) => item.query.query_id === exemplar.query_id)) { setActionError("exemplar_not_found_in_audit"); return; }
      const profile = await doctorApi.fetchConnectedProfile({ connection_id: connectionId, query_id: exemplar.query_id, discovery_generation: occurrences.discovery_generation, kind: "baseline" });
      onOpenDoctor(profile);
    } catch (cause) {
      const id = failureId(cause, "profile_fetch_failed");
      if (id === "connection_expired" || id === "connection_session_expired") onSessionExpired?.();
      setActionError(id);
    } finally {
      setPulling(null);
    }
  };

  const refresh = () => { trends.refresh(); series.refresh(); };
  if (!trends.data) {
    if (trends.error === "history_disabled") return <HistoryOffState onOpenQueries={onOpenQueries} onRetry={() => { history.refresh(); refresh(); }} store="disabled" text={text} />;
    return trends.error ? <MonitorReadError error={trends.error} onRetry={refresh} text={text} title={text("Trends could not be read.", "趋势暂时无法读取。")} /> : <p className="workspace-loading" role="status">{text("Loading…", "正在加载…")}</p>;
  }
  return <>
    {actionError && <p className="capability-error" role="alert">{actionError === "exemplar_not_found_in_audit" ? text("This execution is no longer in the audit table window; its Profile cannot be pulled.", "审计表中已找不到这次执行，无法拉取 Profile。") : governanceActionText(actionError, text)}</p>}
    {series.error && !series.data && selected && <MonitorReadError error={series.error} onRetry={series.refresh} text={text} title={text("This template's trend could not be read.", "该模板的趋势暂时无法读取。")} />}
    {pulling && <p className="workspace-loading" role="status">{text("Pulling Profile…", "正在拉取 Profile…")}</p>}
    <TrendsPanel
      baselineHours={windows.baseline}
      currentHours={windows.current}
      onPullProfile={sameTarget && onOpenDoctor && series.data ? (queryId) => { const exemplar = series.data?.exemplars.find((item) => item.query_id === queryId); if (exemplar && series.data && !pulling) void pull(exemplar, series.data.row); } : undefined}
      onAddToQueue={onOpenItem ? (row) => {
        if (!shown) return;
        setActionError(null);
        doctorApi.createGovernanceItem({ target: shown, template_key: row.template_key, origin: "trends", alert_id: null, title: null })
          .then((item) => onOpenItem(item.item_id))
          .catch((cause: unknown) => setActionError(failureId(cause, "request_failed")));
      } : undefined}
      onSelect={setSelected}
      onWindow={(baseline, current) => setWindows({ baseline, current })}
      selectedKey={selected}
      series={series.data}
      text={text}
      toolbar={<>
        <TargetPicker onTarget={onTarget} target={shown} targets={trends.data.targets} text={text} />
        <RefreshBar error={trends.error ?? series.error} loading={trends.loading || series.loading} onRefresh={refresh} text={text} updatedSeconds={trends.updatedSeconds} />
      </>}
      trends={trends.data}
    />
  </>;
}

export function HistoryWorkspace({ connectionId, history, tab, onTab, target = null, onTarget = () => undefined, onTargets, focusTemplate = null, onOpenDoctor, onOpenItem, onOpenQueries, onSessionExpired }: {
  connectionId: string | null;
  /** The monitored cluster; null lets the server pick the most recently collected one. */
  target?: string | null;
  onTarget?(target: string): void;
  /** Every target the store knows, as the last trends read listed them. */
  onTargets?(targets: string[]): void;
  focusTemplate?: string | null;
  onOpenItem?(itemId: string): void;
  onOpenQueries?(): void;
  tab: MonitoringTab;
  onTab(tab: MonitoringTab): void;
  history: HistoryStatusState;
  onOpenDoctor?(profile: ProfileFetchResponse): void;
  onSessionExpired?(): void;
}): JSX.Element {
  const { text } = useLocale();
  const [busy, setBusy] = useState(false);
  const [confirmClear, setConfirmClear] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const tabs: { id: MonitoringTab; label: string }[] = [
    { id: "trends", label: text("Template trends and regressions", "模板趋势与回归") },
    { id: "collection", label: text("Collection and history", "采集与历史") },
  ];
  const run = (work: () => Promise<void>) => {
    setBusy(true);
    setActionError(null);
    work().catch(() => setActionError(text("The local API rejected the request.", "本地 API 拒绝了该请求。")))
      .finally(() => { setBusy(false); history.refresh(); });
  };
  const blocked = blockedStore(history.status?.store, null);
  return <div className="history-page">
    <div aria-label={text("Monitoring views", "监控视图")} className="history-tabs" role="tablist">
      {tabs.map((item) => <button aria-selected={item.id === tab} key={item.id} onClick={() => onTab(item.id)} role="tab" type="button">{item.label}</button>)}
    </div>
    {tab === "trends" ? blocked ? <HistoryOffState detail={history.status?.store} onOpenQueries={onOpenQueries} onRetry={history.refresh} store={blocked} text={text} />
      : <TrendsTab connectionId={connectionId} focusTemplate={focusTemplate} generation={monitorGeneration(history.status)} history={history} onOpenDoctor={onOpenDoctor} onOpenItem={onOpenItem} onOpenQueries={onOpenQueries} onSessionExpired={onSessionExpired} onTarget={onTarget} onTargets={onTargets} target={target} /> : <>
      {history.error && <MonitorReadError error={history.error} onRetry={history.refresh} text={text} title={text("History status could not be read.", "历史状态暂时无法读取。")} />}
      {actionError && <p className="capability-error" role="alert">{actionError}</p>}
      {!history.status ? !history.error && <p className="workspace-loading" role="status">{text("Loading…", "正在加载…")}</p> : <CollectionPanel
        busy={busy}
        onClear={() => setConfirmClear(true)}
        onToggle={(action) => { if (connectionId) run(() => doctorApi.setHistoryCollection({ connection_id: connectionId, action })); }}
        status={history.status}
        text={text}
        clearConfirm={confirmClear && <div aria-labelledby="history-clear-title" className="history-banner" role="alertdialog">
        <strong id="history-clear-title">{text("Delete all local history?", "删除全部本机历史？")}</strong>{" "}
        {text(
          "This deletes all collected history (templates, hourly buckets, load history and saved error logs, watermarks, reads) and every alert, governance item with its record, and pinned baseline snapshot. It cannot be undone. StarRocks is not touched.",
          "将删除全部采集历史（模板、小时桶、导入历史与保存的错误日志、采集水位、读取记录），以及全部告警、治理项及其处理记录和钉住的基线快照，且无法恢复；不会改动 StarRocks。",
        )}{" "}
        <button className="button danger" disabled={busy} onClick={() => { setConfirmClear(false); run(() => doctorApi.clearHistory()); }} type="button">{text("Delete everything", "全部删除")}</button>{" "}
        <button className="button secondary" onClick={() => setConfirmClear(false)} type="button">{text("Cancel", "取消")}</button>
      </div>}
      />}
    </>}
  </div>;
}
