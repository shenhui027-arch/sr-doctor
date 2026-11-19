// Task 63 (board 9): low-frequency configuration gathered behind the gear —
// connection, notification channels and their delivery log, the local history
// store, and the interface language.
import { lazy, Suspense, useState } from "react";
import type { JSX, ReactNode } from "react";
import { useLocale, type Locale } from "../../i18n/locale";
import { InfoTip } from "../shell/ui";
import { CardDeck } from "../shell/CardDeck";
import { SavedConnectionCard } from "./SavedConnectionCard";
import "./settings.css";

const NotifyWorkspace = lazy(() => import("../history/NotifyWorkspace").then((module) => ({ default: module.NotifyWorkspace })));
const DeliveriesWorkspace = lazy(() => import("../history/NotifyWorkspace").then((module) => ({ default: module.DeliveriesWorkspace })));

export type SettingsSection = "connection" | "notify" | "deliveries" | "history" | "language";

export function SettingsWorkspace({ connectionStatus, onOpenConnection, collection, locale, onLocale, section: shownSection, onSection, initialSection = "notify" }: {
  connectionStatus: string;
  onOpenConnection(): void;
  /** The history store and collector view, rendered by the app with its live status. */
  collection: ReactNode;
  locale: Locale;
  onLocale(locale: Locale): void;
  initialSection?: SettingsSection;
  /** Review: the app owns the shown section, so entries like the collection capsule switch it while Settings is open. */
  section?: SettingsSection;
  onSection?(section: SettingsSection): void;
}): JSX.Element {
  const { text } = useLocale();
  const [ownSection, setOwnSection] = useState<SettingsSection>(initialSection);
  const section = shownSection ?? ownSection;
  const setSection = onSection ?? setOwnSection;
  const sections: [SettingsSection, string][] = [
    ["connection", text("Connection", "连接")],
    ["notify", text("Notifications", "通知")],
    ["deliveries", text("Delivery log", "投递记录")],
    ["history", text("History store", "历史库")],
    ["language", text("Language", "语言")],
  ];
  const loading = <p className="workspace-loading" role="status">{text("Loading…", "正在加载…")}</p>;
  return <div className="settings-page">
    <nav aria-label={text("Settings", "设置")} className="settings-nav">
      <h1>{text("Settings", "设置")}</h1>
      {sections.map(([id, label]) => <button aria-current={section === id ? "page" : undefined} key={id} onClick={() => setSection(id)} type="button">{label}</button>)}
    </nav>
    <div className="settings-body">
      {section === "connection" && <section aria-labelledby="settings-connection" className="settings-section">
        <h2 id="settings-connection">{text("Connection", "连接")}</h2>
        <div className="settings-card settings-row">
          <span className="settings-mono">{connectionStatus}</span>
          <button className="button secondary" onClick={onOpenConnection} type="button">{text("Manage connection", "管理连接")}</button>
        </div>
        <SavedConnectionCard />
      </section>}
      {section === "notify" && <Suspense fallback={loading}>
        <section aria-labelledby="settings-notify" className="settings-section">
          <h2 id="settings-notify">{text("Notification channels", "通知渠道")} <InfoTip label={text("Channels and targets", "渠道与发送对象")}>{text("A channel is how to send (server, cloud account); a target is to whom (mailbox, group bot, phone). Each channel takes several targets.", "渠道 = 怎么发（服务器、平台凭据）；发送对象 = 发给谁（邮箱、群机器人、手机号），每个渠道可以有多个。")}</InfoTip></h2>
          <CardDeck resetKey="notify"><NotifyWorkspace text={text} /></CardDeck>
        </section>
      </Suspense>}
      {section === "deliveries" && <Suspense fallback={loading}>
        <section aria-labelledby="settings-deliveries" className="settings-section">
          <h2 id="settings-deliveries">{text("Delivery log", "投递记录")}</h2>
          <CardDeck resetKey="deliveries"><DeliveriesWorkspace text={text} /></CardDeck>
        </section>
      </Suspense>}
      {section === "history" && <section aria-labelledby="settings-history" className="settings-section">
        <h2 id="settings-history">{text("History store and collection", "历史库与采集")}</h2>
        <CardDeck resetKey="history">{collection}</CardDeck>
      </section>}
      {section === "language" && <section aria-labelledby="settings-language" className="settings-section">
        <h2 id="settings-language">{text("Language", "语言")}</h2>
        <div aria-label={text("Language", "语言")} className="locale-switch" role="group">
          <button aria-pressed={locale === "zh"} className={locale === "zh" ? "active" : ""} onClick={() => onLocale("zh")} type="button">中文</button>
          <button aria-pressed={locale === "en"} className={locale === "en" ? "active" : ""} onClick={() => onLocale("en")} type="button">English</button>
        </div>
      </section>}
    </div>
  </div>;
}
