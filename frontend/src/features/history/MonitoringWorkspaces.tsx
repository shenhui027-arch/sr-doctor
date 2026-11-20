// Stage D workspaces: they read the local API and pass the user's choices
// back to it. Nothing here reaches StarRocks.
import { useEffect, useState } from "react";
import type { JSX, ReactNode } from "react";
import type {
  AlertView,
  GovernanceItem,
  HistoryStoreState,
  LoadAlertsMore,
} from "../../../../contracts/generated/doctor-contracts";
import { InfoTip, SegTabs } from "../../design";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { doctorApi } from "../../lib/api";
import { AlertsPanel } from "./AlertsPanel";
import { type GovernanceCommand, GovernancePanel, type NoteDrafts } from "./GovernancePanel";
import { type AlertTab, LoadAlertsPanel, type LoadHistoryTarget } from "./LoadAlertsPanel";
import { blockedStore, HistoryOffState, MonitorReadError, RefreshBar, TargetPicker } from "./MonitorStates";
import { failureId, useMonitorRead } from "./monitor-read";
import "./alerts.css";

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

export type AlertCategory = "sql" | "load";

/** The load alert tab and "show more" of the cluster they were chosen on. */
export type LoadView = { target: string | null; tab: AlertTab | null; more: LoadAlertsMore | null };

/** Another cluster starts from its defaults, whichever picker switched it;
 *  the old view is dropped, so switching back does not bring it back. */
export function loadViewAt(view: LoadView, target: string | null): LoadView {
  return view.target === target ? view : { target, tab: null, more: null };
}

export function AlertsWorkspace({
  target,
  targets = [],
  onTarget,
  store,
  generation = "",
  onRetryStatus,
  onOpenQueries,
  onChanged,
  onOpenTrends,
  onOpenItem,
  initialCategory = "sql",
  onOpenLoadHistory,
  leading = null,
}: MonitorContext & {
  /** Collector progress; a change re-reads the list. */
  generation?: string;
  /** Refreshes the header badge after a change. */
  onChanged(): void;
  onOpenTrends(templateKey: string, target: string): void;
  onOpenItem(itemId: string): void;
  /** Task 59: which kind of alerts opens first. */
  initialCategory?: AlertCategory;
  onOpenLoadHistory?(target: LoadHistoryTarget): void;
  /** View switch (Alerts vs Governance), rendered in the single equal-height toolbar. */
  leading?: ReactNode;
}): JSX.Element {
  const { text } = useLocale();
  const blocked = blockedStore(store, null);
  const alerts = useMonitorRead(
    blocked ? null : (signal) => doctorApi.readHistoryAlerts(target, signal),
    target ?? "",
    generation,
    "history_alerts_failed",
  );
  // "Show more" of one state; every state lists its newest alerts otherwise.
  // The load tab and "show more" belong to one cluster: switching it from any
  // picker (SQL or loads) drops them.
  const [loadState, setLoadState] = useState<LoadView>({ target, tab: null, more: null });
  const loadView = loadViewAt(loadState, target);
  if (loadView !== loadState) setLoadState(loadView);
  const { tab: loadTab, more: loadMore } = loadView;
  const setLoadMore = (more: LoadAlertsMore) => setLoadState({ target, tab: loadTab, more });
  const setLoadTab = (tab: AlertTab) => setLoadState({ target, tab, more: loadMore });
  const loadAlerts = useMonitorRead(
    blocked ? null : (signal) => doctorApi.readLoadAlerts(target, signal, loadMore),
    `load:${target ?? ""}:${loadMore ? `${loadMore.state}:${loadMore.limit}` : ""}`,
    generation,
    "load_alerts_failed",
  );
  const [category, setCategory] = useState<AlertCategory>(initialCategory);
  const [selectedLoad, setSelectedLoad] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Kept apart from read errors: a reload after a failed action must not hide why it failed.
  const [actionError, setActionError] = useState<string | null>(null);

  const run = (work: () => Promise<unknown>, after?: (value: unknown) => void) => {
    setBusy(true);
    setActionError(null);
    work()
      .then((value) => after?.(value))
      .catch((cause: unknown) => setActionError(failureId(cause, "request_failed")))
      .finally(() => {
        setBusy(false);
        alerts.refresh();
        loadAlerts.refresh();
        onChanged();
      });
  };

  const off = blocked ?? blockedStore(undefined, alerts.data ? null : alerts.error);
  if (off) {
    return (
      <div className="alerts-stage">
        {leading ? <div className="workspace-bar">{leading}</div> : null}
        <HistoryOffState
          detail={store}
          onOpenQueries={onOpenQueries}
          onRetry={() => {
            onRetryStatus();
            alerts.refresh();
          }}
          store={off}
          text={text}
        />
      </div>
    );
  }

  const categoryControls = (
    <>
      {leading}
      <SegTabs<AlertCategory>
        aria-label={text("Alert category", "告警类别")}
        items={[
          {
            value: "sql",
            label: text("SQL regressions", "SQL 性能回归"),
            count: alerts.data?.counts.open ?? "—",
          },
          {
            value: "load",
            label: text("Load alerts", "导入异常"),
            count: loadAlerts.data?.counts.open ?? "—",
          },
        ]}
        onValueChange={setCategory}
        value={category}
      />
      <InfoTip label={text("About the counts", "关于条数")}>
        {text(
          "Numbers are new alerts (not yet acknowledged).",
          "数字为新告警条数（未确认）。",
        )}
      </InfoTip>
    </>
  );

  if (category === "load") {
    if (!loadAlerts.data) {
      return (
        <div className="alerts-stage">
          <div className="workspace-bar">{categoryControls}</div>
          {loadAlerts.error ? (
            <MonitorReadError
              error={loadAlerts.error}
              onRetry={loadAlerts.refresh}
              text={text}
              title={text("Load alerts could not be read.", "导入告警暂时无法读取。")}
            />
          ) : (
            <div className="state-box" role="status">{text("Loading…", "正在加载…")}</div>
          )}
        </div>
      );
    }
    return (
      <div className="alerts-stage">
        {actionError && (
          <div className="notice-banner notice-banner--danger" role="alert">
            {governanceActionText(actionError, text)}
          </div>
        )}
        <LoadAlertsPanel
          alerts={loadAlerts.data}
          busy={busy}
          onAction={(id, action) => run(() => doctorApi.loadAlertAction({ alert_id: id, action }))}
          onMore={setLoadMore}
          onOpenHistory={(focus) => onOpenLoadHistory?.(focus)}
          onSelect={setSelectedLoad}
          onTab={setLoadTab}
          selectedId={selectedLoad}
          tab={loadTab}
          text={text}
          toolbar={
            <>
              <TargetPicker onTarget={onTarget} target={loadAlerts.data.target} targets={targets} text={text} />
              <RefreshBar
                error={loadAlerts.error}
                loading={loadAlerts.loading}
                onRefresh={loadAlerts.refresh}
                text={text}
                updatedSeconds={loadAlerts.updatedSeconds}
              />
            </>
          }
          toolbarLeading={categoryControls}
        />
      </div>
    );
  }

  if (!alerts.data) {
    return (
      <div className="alerts-stage">
        <div className="workspace-bar">{categoryControls}</div>
        {alerts.error ? (
          <MonitorReadError
            error={alerts.error}
            onRetry={alerts.refresh}
            text={text}
            title={text("Alerts could not be read.", "告警暂时无法读取。")}
          />
        ) : (
          <div className="state-box" role="status">{text("Loading…", "正在加载…")}</div>
        )}
      </div>
    );
  }

  return (
    <div className="alerts-stage">
      {actionError && (
        <div className="notice-banner notice-banner--danger" role="alert">
          {governanceActionText(actionError, text)}
        </div>
      )}
      <AlertsPanel
        alerts={alerts.data}
        busy={busy}
        onAction={(id, action) => run(() => doctorApi.alertAction({ alert_id: id, action }))}
        onAddToQueue={(alert: AlertView) =>
          run(
            () =>
              doctorApi.createGovernanceItem({
                target: alert.target,
                template_key: alert.template_key,
                origin: "alert",
                alert_id: alert.alert_id,
                title: null,
              }),
            (item) => onOpenItem((item as GovernanceItem).item_id),
          )
        }
        onOpenItem={onOpenItem}
        onOpenTrends={(templateKey) => {
          const alert = alerts.data?.alerts.find((item) => item.template_key === templateKey);
          onOpenTrends(templateKey, alert?.target ?? alerts.data?.target ?? "");
        }}
        onResolveRuleChanged={() => run(() => doctorApi.resolveRuleChangedAlerts(alerts.data?.target ?? null))}
        onSelect={setSelected}
        selectedId={selected}
        text={text}
        toolbar={
          <>
            <TargetPicker onTarget={onTarget} target={alerts.data.target} targets={targets} text={text} />
            <RefreshBar
              error={alerts.error}
              loading={alerts.loading}
              onRefresh={alerts.refresh}
              text={text}
              updatedSeconds={alerts.updatedSeconds}
            />
          </>
        }
        toolbarLeading={categoryControls}
      />
    </div>
  );
}

/** Why an alert or governance action was refused, in words; unknown codes keep the code. */
export function governanceActionText(code: string, text: LocalizedText): string {
  return (
    {
      illegal_governance_transition: text("That step is not allowed from the current state.", "当前状态不允许这一步。"),
      snapshot_required: text("Pin a baseline snapshot first.", "请先钉住基线快照。"),
      no_current_samples_to_pin: text("The template has no recent executions to pin.", "该模板最近没有执行，无法钉住。"),
      governance_item_already_open: text(
        "This template already has an open governance item; finish or close that one first.",
        "该模板已有一个进行中的治理项，请先处理或关闭那一项。",
      ),
    }[code] ?? `${text("Request failed", "请求失败")} (${code})`
  );
}

/** Drops an item's draft once its change is recorded; a failed submit keeps it for a retry. */
export function settleNote(notes: NoteDrafts, itemId: string, recorded: boolean): NoteDrafts {
  if (!recorded || !(itemId in notes)) return notes;
  const next = { ...notes };
  delete next[itemId];
  return next;
}

export function GovernanceWorkspace({
  focusId,
  target,
  targets = [],
  onTarget,
  store,
  onRetryStatus,
  onOpenQueries,
  onOpenTrends,
  leading = null,
}: MonitorContext & {
  focusId: string | null;
  onOpenTrends(templateKey: string, target: string): void;
  /** View switch (Alerts vs Governance), rendered in the single equal-height toolbar. */
  leading?: ReactNode;
}): JSX.Element {
  const { text } = useLocale();
  const blocked = blockedStore(store, null);
  // A work queue: read on entry, after each action and on request, not on every collection.
  const governance = useMonitorRead(
    blocked ? null : (signal) => doctorApi.readGovernance(target, signal),
    target ?? "",
    "",
    "governance_failed",
  );
  const [selected, setSelected] = useState<string | null>(focusId);
  // An alert's "Open governance item #N" opens that item's drawer once.
  const [openRequest, setOpenRequest] = useState<string | null>(focusId);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [notes, setNotes] = useState<NoteDrafts>({});

  useEffect(() => {
    if (focusId) {
      setSelected(focusId);
      setOpenRequest(focusId);
    }
  }, [focusId]);

  const command = (item: GovernanceItem, next: GovernanceCommand) => {
    setBusy(true);
    setActionError(null);
    const request = { item_id: item.item_id };
    const work =
      next.kind === "transition"
        ? doctorApi.transitionGovernanceItem({ ...request, to: next.to, note: null })
        : next.kind === "pin"
          ? doctorApi.pinGovernanceSnapshot(request)
          : next.kind === "change"
            ? doctorApi.recordGovernanceChange({ ...request, note: next.note.trim(), change_seconds: null })
            : doctorApi.verifyGovernanceItem(request);
    work
      .then(() => {
        if (next.kind === "change") setNotes((value) => settleNote(value, item.item_id, true));
      })
      .catch((cause: unknown) => setActionError(failureId(cause, "request_failed")))
      .finally(() => {
        setBusy(false);
        governance.refresh();
      });
  };

  const off = blocked ?? blockedStore(undefined, governance.data ? null : governance.error);
  if (off) {
    return (
      <div className="alerts-stage">
        {leading ? <div className="workspace-bar">{leading}</div> : null}
        <HistoryOffState
          detail={store}
          onOpenQueries={onOpenQueries}
          onRetry={() => {
            onRetryStatus();
            governance.refresh();
          }}
          store={off}
          text={text}
        />
      </div>
    );
  }

  if (!governance.data) {
    return (
      <div className="alerts-stage">
        {leading ? <div className="workspace-bar">{leading}</div> : null}
        {governance.error ? (
          <MonitorReadError
            error={governance.error}
            onRetry={governance.refresh}
            text={text}
            title={text("The governance queue could not be read.", "治理队列暂时无法读取。")}
          />
        ) : (
          <div className="state-box" role="status">{text("Loading…", "正在加载…")}</div>
        )}
      </div>
    );
  }

  const shown = target ?? governance.data.items[0]?.target ?? null;
  return (
    <div className="alerts-stage">
      {actionError && (
        <div className="notice-banner notice-banner--danger" role="alert">
          {governanceActionText(actionError, text)}
        </div>
      )}
      <GovernancePanel
        busy={busy}
        governance={governance.data}
        notes={notes}
        onCommand={command}
        onNote={(itemId, note) => setNotes((value) => ({ ...value, [itemId]: note }))}
        onOpenTrends={(templateKey) => {
          const item = governance.data?.items.find((value) => value.template_key === templateKey);
          onOpenTrends(templateKey, item?.target ?? shown ?? "");
        }}
        onOpenedItem={() => setOpenRequest(null)}
        onSelect={setSelected}
        openItemId={openRequest}
        selectedId={selected}
        text={text}
        toolbar={
          <>
            <TargetPicker onTarget={onTarget} target={shown} targets={targets} text={text} />
            <RefreshBar
              error={governance.error}
              loading={governance.loading}
              onRefresh={governance.refresh}
              text={text}
              updatedSeconds={governance.updatedSeconds}
            />
          </>
        }
        toolbarLeading={leading}
      />
    </div>
  );
}
