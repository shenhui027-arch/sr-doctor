// Task 81 (boards/08-settings.html): three sections in a top SegTabs bar —
// 1. 历史库与采集 (mounts CollectionPanel directly, no HistoryWorkspace / CardDeck)
// 2. 告警通知 (channels, targets & delivery log; NotifyDialog uses Dialog)
// 3. 连接与偏好 (saved-connection card + language & security boundaries card, decided item 4)
import { lazy, Suspense, useState } from "react";
import type { JSX, ReactNode } from "react";
import type { TlsMode } from "../../../../contracts/generated/doctor-contracts";
import { BadgeStamp, Card, PageGrid, SegTabs, type SegItem } from "../../design";
import { useLocale, type Locale } from "../../i18n/locale";
import { doctorApi } from "../../lib/api";
import { CollectionPanel } from "../history/CollectionPanel";
import type { HistoryStatusState } from "../history/history-status";
import { MonitorReadError } from "../history/MonitorStates";
import { SavedConnectionCard } from "./SavedConnectionCard";
import "./settings.css";

const NotifyWorkspace = lazy(() => import("../history/NotifyWorkspace").then((module) => ({ default: module.NotifyWorkspace })));

export type SettingsSection = "history" | "notify" | "connection" | "deliveries" | "language";
type PrimarySettingsSection = "history" | "notify" | "connection";

function normalizeSection(section: SettingsSection): PrimarySettingsSection {
  if (section === "deliveries") return "notify";
  if (section === "language") return "connection";
  return section;
}

export function SettingsWorkspace({
  connectionStatus,
  tlsMode = null,
  onOpenConnection,
  history,
  connectionId = null,
  collection,
  locale,
  onLocale,
  section: shownSection,
  onSection,
  initialSection = "history",
}: {
  connectionStatus: string;
  tlsMode?: TlsMode | null;
  onOpenConnection(): void;
  history?: HistoryStatusState;
  connectionId?: string | null;
  /** Optional legacy override; when `history` is provided, `CollectionPanel` is mounted directly. */
  collection?: ReactNode;
  locale: Locale;
  onLocale(locale: Locale): void;
  initialSection?: SettingsSection;
  section?: SettingsSection;
  onSection?(section: SettingsSection): void;
}): JSX.Element {
  const { text } = useLocale();
  const [ownSection, setOwnSection] = useState<SettingsSection>(initialSection);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  const rawSection = shownSection ?? ownSection;
  const activeSection = normalizeSection(rawSection);
  const setSection = onSection ?? setOwnSection;

  const sectionItems: readonly SegItem<PrimarySettingsSection>[] = [
    { value: "history", label: text("History store and collection", "历史库与采集") },
    { value: "notify", label: text("Alert notifications", "告警通知") },
    { value: "connection", label: text("Connection and preferences", "连接与偏好") },
  ];

  const localeItems: readonly SegItem<Locale>[] = [
    { value: "zh", label: "中文" },
    { value: "en", label: "English" },
  ];

  const runHistoryAction = (work: () => Promise<void>) => {
    setBusy(true);
    setActionError(null);
    work()
      .catch(() => setActionError(text("The local API rejected the request.", "本地 API 拒绝了该请求。")))
      .finally(() => {
        setBusy(false);
        history?.refresh();
      });
  };

  const store = history?.status?.store;
  const remoteEnabled = history?.status?.remote_collection_enabled ?? false;
  const loading = <p className="workspace-loading" role="status">{text("Loading…", "正在加载…")}</p>;

  return (
    <div className="settings-stage">
      <div className="workspace-bar">
        <SegTabs
          aria-label={text("Settings sections", "设置分区切换")}
          items={sectionItems}
          onValueChange={(next) => setSection(next)}
          value={activeSection}
        />

        <div className="workspace-bar-right">
          {store?.state === "available" ? (
            <span className="status-inline">
              <span aria-hidden="true" className="status-dot status-dot--ok" />
              {text(`SQLite history available · ${store.retention_days}d retention`, `SQLite 历史库可用 · ${store.retention_days} 天保留`)}
            </span>
          ) : store?.state === "disabled" ? (
            <span className="state-stamp">{text("History store off", "历史库已关闭")}</span>
          ) : null}
          {!remoteEnabled ? (
            <span
              className="state-stamp state-stamp--warn"
              title={text(
                "Remote production background collection stays off until the G1 read-only pilot passes",
                "G1 生产只读试点尚未实测，远端生产集群后台采集默认关闭",
              )}
            >
              {text("Remote G1 pilot · pending", "远端 G1 试点 · 待验证")}
            </span>
          ) : null}
        </div>
      </div>

      {activeSection === "history" ? (
        history ? (
          <>
            {history.error ? (
              <MonitorReadError
                error={history.error}
                onRetry={history.refresh}
                text={text}
                title={text("History status could not be read.", "历史状态暂时无法读取。")}
              />
            ) : null}
            {actionError ? (
              <div className="notice-banner notice-banner--danger" role="alert">
                <span>{actionError}</span>
              </div>
            ) : null}
            {!history.status ? (
              !history.error ? loading : null
            ) : (
              <CollectionPanel
                busy={busy}
                onClear={() => runHistoryAction(() => doctorApi.clearHistory())}
                onToggle={(action) => runHistoryAction(() => doctorApi.setHistoryCollection({ connection_id: connectionId, action }))}
                status={history.status}
                text={text}
              />
            )}
          </>
        ) : (
          <>{collection}</>
        )
      ) : null}

      {activeSection === "notify" ? (
        <Suspense fallback={loading}>
          <NotifyWorkspace
            initialRightTab={rawSection === "deliveries" ? "deliveries" : "targets"}
            text={text}
          />
        </Suspense>
      ) : null}

      {activeSection === "connection" ? (
        <PageGrid className="settings-conn-grid">
          <SavedConnectionCard
            connectionStatus={connectionStatus}
            onOpenConnection={onOpenConnection}
            span={{ lg: 7, md: 8, sm: 4 }}
            tlsMode={tlsMode}
          />

          <Card
            footer={<span>{text("SQL and raw Profiles are never uploaded to external services", "不向任何外部服务上传 SQL 或 Profile 原文")}</span>}
            span={{ lg: 5, md: 8, sm: 4 }}
            title={text("Language and security boundaries", "界面语言与安全边界")}
            variant="form"
          >
            <div className="settings-pref-stack">
              <div className="field">
                <span className="field-label">{text("Interface language", "界面显示语言")}</span>
                <div className="settings-locale-row">
                  <SegTabs
                    aria-label={text("Language", "界面语言切换")}
                    items={localeItems}
                    onValueChange={onLocale}
                    value={locale}
                  />
                  <span className="card-subtitle">
                    {locale === "zh"
                      ? "当前：简体中文（外发通知语言独立设置）"
                      : "Current: English (outbound alert language stays independent)"}
                  </span>
                </div>
              </div>

              <div className="settings-pref-subblock">
                <div className="field-label settings-pref-subhead">{text("Security and product boundaries", "会话与产品安全摘要")}</div>
                <dl className="kv-grid">
                  <dt>{text("Write protection", "写操作防护")}</dt>
                  <dd>{text("Same-origin check + CSRF nonce", "强制同源 Origin 校验 + CSRF Nonce")}</dd>
                  <dt>{text("Remote collection gate", "远端采集门禁")}</dt>
                  <dd>
                    <span className="state-stamp state-stamp--warn">
                      {text("Pending · off before G1 pilot", "待验证 · G1 生产只读试点通过前关闭")}
                    </span>
                  </dd>
                  <dt>{text("Multi-user / RBAC", "多用户 / RBAC")}</dt>
                  <dd><BadgeStamp kind="planned" /></dd>
                </dl>
              </div>
            </div>
          </Card>
        </PageGrid>
      ) : null}
    </div>
  );
}
