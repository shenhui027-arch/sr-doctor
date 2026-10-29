// The notification tabs of the alerts workspace: they read the local API
// and send the user's edits back. Nothing here reaches StarRocks.
import { useState } from "react";
import type { JSX } from "react";
import type { NotifyChannelSaveRequest, NotifyOverviewResponse, NotifyTargetInput } from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { doctorApi } from "../../lib/api";
import { failureId, useMonitorRead } from "./monitor-read";
import { MonitorReadError, RefreshBar } from "./MonitorStates";
import { NotifyDialog, type DialogFocus } from "./NotifyDialog";
import { DeliveriesPanel, NotifyPanel, type DeliveryFilters, type TestResults } from "./NotifyPanel";
import { notifyActionText, targetInput } from "./notify-text";

function useTests(): [TestResults, (input: NotifyTargetInput, key: string) => void] {
  const [tests, setTests] = useState<TestResults>({});
  const run = (input: NotifyTargetInput, key: string) => {
    setTests((current) => ({ ...current, [key]: "running" }));
    doctorApi.testNotifyTarget(input)
      .then((result) => setTests((current) => ({ ...current, [key]: result })))
      .catch((cause: unknown) => setTests((current) => ({ ...current, [key]: { state: "failed", error: null, provider_code: failureId(cause, "request_failed"), duration_ms: 0 } })));
  };
  return [tests, run];
}

export function NotifyWorkspace({ text }: { text: LocalizedText }): JSX.Element {
  const read = useMonitorRead((signal) => doctorApi.readNotifyOverview(signal), "notify", "", "notify_overview_failed");
  const [saved, setSaved] = useState<NotifyOverviewResponse | null>(null);
  const [focus, setFocus] = useState<DialogFocus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [tests, test] = useTests();
  const overview = saved ?? read.data;
  // Every change answers with the fresh overview; a refusal keeps its reason.
  const change = (work: () => Promise<NotifyOverviewResponse>): Promise<boolean> => {
    setBusy(true);
    setError(null);
    return work()
      .then((next) => { setSaved(next); return true; })
      .catch((cause: unknown) => { setError(failureId(cause, "request_failed")); return false; })
      .finally(() => setBusy(false));
  };
  if (!overview) {
    return read.error
      ? <MonitorReadError error={read.error} onRetry={read.refresh} text={text} title={read.error === "notify_key_unavailable" || read.error === "history_disabled" ? notifyActionText(read.error, text) : text("Notification settings could not be read.", "通知设置暂时无法读取。")} />
      : <p className="workspace-loading" role="status">{text("Loading…", "正在加载…")}</p>;
  }
  return <>
    {error && !focus && <p className="capability-error" role="alert">{notifyActionText(error, text)}</p>}
    <NotifyPanel
      busy={busy}
      key={JSON.stringify(overview.settings)}
      onOpen={(kind, targetId) => { setError(null); setFocus({ kind, edit: targetId ?? null }); }}
      onSaveSettings={(settings) => void change(() => doctorApi.saveNotifySettings(settings))}
      onTest={(target) => test(targetInput(target), target.target_id)}
      overview={overview}
      tests={tests}
      text={text}
    />
    {focus && <NotifyDialog
      busy={busy}
      error={error}
      focus={focus}
      key={`${focus.kind}:${focus.edit ?? ""}`}
      onClose={() => { setFocus(null); setError(null); }}
      onDeleteTarget={(targetId) => void change(() => doctorApi.deleteNotifyTarget(targetId))}
      onSaveChannel={(request: NotifyChannelSaveRequest) => change(() => doctorApi.saveNotifyChannel(request))}
      onSaveTarget={(input) => change(() => doctorApi.saveNotifyTarget(input))}
      onTest={test}
      overview={overview}
      tests={tests}
      text={text}
    />}
  </>;
}

export function DeliveriesWorkspace({ text }: { text: LocalizedText }): JSX.Element {
  const [filters, setFilters] = useState<DeliveryFilters>({ kind: null, target_id: null, state: null });
  const scope = JSON.stringify(filters);
  const deliveries = useMonitorRead((signal) => doctorApi.readNotifyDeliveries(filters, signal), scope, "", "notify_deliveries_failed");
  const overview = useMonitorRead((signal) => doctorApi.readNotifyOverview(signal), "notify", "", "notify_overview_failed");
  if (!deliveries.data) {
    return deliveries.error
      ? <MonitorReadError error={deliveries.error} onRetry={deliveries.refresh} text={text} title={deliveries.error === "notify_key_unavailable" || deliveries.error === "history_disabled" ? notifyActionText(deliveries.error, text) : text("The delivery log could not be read.", "投递记录暂时无法读取。")} />
      : <p className="workspace-loading" role="status">{text("Loading…", "正在加载…")}</p>;
  }
  return <>
    <span className="trend-toolbar notify-refresh"><RefreshBar error={deliveries.error} loading={deliveries.loading} onRefresh={deliveries.refresh} text={text} updatedSeconds={deliveries.updatedSeconds} /></span>
    <DeliveriesPanel deliveries={deliveries.data} filters={filters} onFilters={setFilters} targets={overview.data?.targets ?? []} text={text} />
  </>;
}
