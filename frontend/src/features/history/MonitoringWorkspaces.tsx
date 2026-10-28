// Stage D workspaces: they read the local API and pass the user's choices
// back to it. Nothing here reaches StarRocks.
import { useEffect, useState } from "react";
import type { JSX } from "react";
import type { AlertView, GovernanceItem, HistoryStoreState } from "../../../../contracts/generated/doctor-contracts";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { doctorApi } from "../../lib/api";
import { AlertsPanel } from "./AlertsPanel";
import { GovernancePanel, type GovernanceCommand, type NoteDrafts } from "./GovernancePanel";
import { failureId, useMonitorRead } from "./monitor-read";
import { blockedStore, HistoryOffState, MonitorReadError, RefreshBar, TargetPicker } from "./MonitorStates";
import "./history.css";

type MonitorContext = {
  /** The monitored cluster; null lets the server pick the most recently collected one. */
  target: string | null;
  /** Targets known from the trends view; a picker appears when there are several. */
  targets?: string[];
  onTarget?(target: string): void;
  /** The history store as the status poll last saw it. */
  store: HistoryStoreState | undefined;
  onRetryStatus(): void;
  onOpenQueries?(): void;
};

export function AlertsWorkspace({ target, targets = [], onTarget, store, generation = "", onRetryStatus, onOpenQueries, onChanged, onOpenTrends, onOpenItem }: MonitorContext & {
  /** Collector progress; a change re-reads the list. */
  generation?: string;
  /** Refreshes the header badge after a change. */
  onChanged(): void;
  onOpenTrends(templateKey: string, target: string): void;
  onOpenItem(itemId: string): void;
}): JSX.Element {
  const { text } = useLocale();
  const blocked = blockedStore(store, null);
  const alerts = useMonitorRead(blocked ? null : (signal) => doctorApi.readHistoryAlerts(target, signal), target ?? "", generation, "history_alerts_failed");
  const [selected, setSelected] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Kept apart from read errors: a reload after a failed action must not hide why it failed.
  const [actionError, setActionError] = useState<string | null>(null);
  const run = (work: () => Promise<unknown>, after?: (value: unknown) => void) => {
    setBusy(true);
    setActionError(null);
    work().then((value) => after?.(value)).catch((cause: unknown) => setActionError(failureId(cause, "request_failed")))
      .finally(() => { setBusy(false); alerts.refresh(); onChanged(); });
  };
  const off = blocked ?? blockedStore(undefined, alerts.data ? null : alerts.error);
  if (off) return <HistoryOffState onOpenQueries={onOpenQueries} onRetry={() => { onRetryStatus(); alerts.refresh(); }} detail={store} store={off} text={text} />;
  if (!alerts.data) return alerts.error ? <MonitorReadError error={alerts.error} onRetry={alerts.refresh} text={text} title={text("Alerts could not be read.", "告警暂时无法读取。")} /> : <p className="workspace-loading" role="status">{text("Loading…", "正在加载…")}</p>;
  return <>
    {actionError && <p className="capability-error" role="alert">{governanceActionText(actionError, text)}</p>}
    <AlertsPanel
      alerts={alerts.data}
      busy={busy}
      onAction={(id, action) => run(() => doctorApi.alertAction({ alert_id: id, action }))}
      onAddToQueue={(alert: AlertView) => run(
        () => doctorApi.createGovernanceItem({ target: alert.target, template_key: alert.template_key, origin: "alert", alert_id: alert.alert_id, title: null }),
        (item) => onOpenItem((item as GovernanceItem).item_id),
      )}
      onOpenItem={onOpenItem}
      onOpenTrends={(templateKey) => { const alert = alerts.data?.alerts.find((item) => item.template_key === templateKey); onOpenTrends(templateKey, alert?.target ?? alerts.data?.target ?? ""); }}
      onSelect={setSelected}
      selectedId={selected}
      text={text}
      toolbar={<>
        <TargetPicker onTarget={onTarget} target={alerts.data.target} targets={targets} text={text} />
        <RefreshBar error={alerts.error} loading={alerts.loading} onRefresh={alerts.refresh} text={text} updatedSeconds={alerts.updatedSeconds} />
      </>}
    />
  </>;
}

/** Why an alert or governance action was refused, in words; unknown codes keep the code. */
export function governanceActionText(code: string, text: LocalizedText): string {
  return {
    illegal_governance_transition: text("That step is not allowed from the current state.", "当前状态不允许这一步。"),
    snapshot_required: text("Pin a baseline snapshot first.", "请先钉住基线快照。"),
    no_current_samples_to_pin: text("The template has no recent executions to pin.", "该模板最近没有执行，无法钉住。"),
    governance_item_already_open: text("This template already has an open governance item; finish or close that one first.", "该模板已有一个进行中的治理项，请先处理或关闭那一项。"),
  }[code] ?? `${text("Request failed", "请求失败")} (${code})`;
}

/** Drops an item's draft once its change is recorded; a failed submit keeps it for a retry. */
export function settleNote(notes: NoteDrafts, itemId: string, recorded: boolean): NoteDrafts {
  if (!recorded || !(itemId in notes)) return notes;
  const next = { ...notes };
  delete next[itemId];
  return next;
}

export function GovernanceWorkspace({ focusId, target, targets = [], onTarget, store, onRetryStatus, onOpenQueries, onOpenTrends }: MonitorContext & {
  focusId: string | null;
  onOpenTrends(templateKey: string, target: string): void;
}): JSX.Element {
  const { text } = useLocale();
  const blocked = blockedStore(store, null);
  // A work queue: read on entry, after each action and on request, not on every collection.
  const governance = useMonitorRead(blocked ? null : (signal) => doctorApi.readGovernance(target, signal), target ?? "", "", "governance_failed");
  const [selected, setSelected] = useState<string | null>(focusId);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [notes, setNotes] = useState<NoteDrafts>({});
  useEffect(() => { if (focusId) setSelected(focusId); }, [focusId]);
  const command = (item: GovernanceItem, next: GovernanceCommand) => {
    setBusy(true);
    setActionError(null);
    const request = { item_id: item.item_id };
    const work = next.kind === "transition" ? doctorApi.transitionGovernanceItem({ ...request, to: next.to, note: null })
      : next.kind === "pin" ? doctorApi.pinGovernanceSnapshot(request)
      : next.kind === "change" ? doctorApi.recordGovernanceChange({ ...request, note: next.note.trim(), change_seconds: null })
      : doctorApi.verifyGovernanceItem(request);
    work.then(() => { if (next.kind === "change") setNotes((value) => settleNote(value, item.item_id, true)); })
      .catch((cause: unknown) => setActionError(failureId(cause, "request_failed")))
      .finally(() => { setBusy(false); governance.refresh(); });
  };
  const off = blocked ?? blockedStore(undefined, governance.data ? null : governance.error);
  if (off) return <HistoryOffState onOpenQueries={onOpenQueries} onRetry={() => { onRetryStatus(); governance.refresh(); }} detail={store} store={off} text={text} />;
  if (!governance.data) return governance.error ? <MonitorReadError error={governance.error} onRetry={governance.refresh} text={text} title={text("The governance queue could not be read.", "治理队列暂时无法读取。")} /> : <p className="workspace-loading" role="status">{text("Loading…", "正在加载…")}</p>;
  const shown = target ?? governance.data.items[0]?.target ?? null;
  return <>
    {actionError && <p className="capability-error" role="alert">{governanceActionText(actionError, text)}</p>}
    <GovernancePanel
      busy={busy}
      governance={governance.data}
      notes={notes}
      onCommand={command}
      onNote={(itemId, note) => setNotes((value) => ({ ...value, [itemId]: note }))}
      onOpenTrends={(templateKey) => { const item = governance.data?.items.find((value) => value.template_key === templateKey); onOpenTrends(templateKey, item?.target ?? shown ?? ""); }}
      onSelect={setSelected}
      selectedId={selected}
      text={text}
      toolbar={<>
        <TargetPicker onTarget={onTarget} target={shown} targets={targets} text={text} />
        <RefreshBar error={governance.error} loading={governance.loading} onRefresh={governance.refresh} text={text} updatedSeconds={governance.updatedSeconds} />
      </>}
    />
  </>;
}
