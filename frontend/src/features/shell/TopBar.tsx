import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import type { JSX, RefObject } from "react";
import type { HistoryStatusResponse } from "../../../../contracts/generated/doctor-contracts";
import { BadgeStamp, Icon, Tip } from "../../design";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import type { ConnectionState } from "../connection/connection-state";
import { CollectionCapsule } from "../history/history-status";
import { HeaderStatusText } from "./HeaderStatusText";

export type TopBarWorkspace = "Overview" | "Queries" | "Doctor" | "Alerts" | "Loads" | "Settings";

const PRIMARY_WORKSPACES = ["Overview", "Queries", "Doctor"] as const;
const OVERFLOW_WORKSPACES = ["Alerts", "Loads"] as const;

export function workspaceLabel(workspace: TopBarWorkspace, text: LocalizedText): string {
  const labels: Record<TopBarWorkspace, readonly [string, string]> = {
    Overview: ["Overview", "总览"],
    Queries: ["Queries", "查询"],
    Doctor: ["Doctor", "诊断"],
    Alerts: ["Alerts", "告警"],
    Loads: ["Loads", "导入"],
    Settings: ["Settings", "设置"],
  };
  return text(...labels[workspace]);
}

export function localizedSessionStatus(connection: ConnectionState, text: LocalizedText): string {
  if ((connection.phase === "ready" || connection.phase === "unsupported") && connection.session) {
    const version = connection.session.detected_version?.value ?? text("version unknown", "版本未知");
    return `${connection.phase === "ready" ? text("Connected", "已连接") : text("Limited", "受限")} · ${version} · ${text("read-only", "只读")}`;
  }
  if (connection.phase === "connecting") return text("Connecting…", "连接中…");
  if (connection.phase === "expired") {
    return connection.message === "remote_timeout"
      ? text("Query timed out · reconnect", "查询超时已断开 · 请重连")
      : text("Session expired · reconnect", "会话已过期 · 请重连");
  }
  if (connection.phase === "failed") return text("Connection failed", "连接失败");
  return text("Not connected", "未连接");
}

function connectionDotClass(connection: ConnectionState): string {
  if (connection.phase === "ready" && connection.session) return "status-dot status-dot--ok";
  if (connection.phase === "unsupported" || connection.phase === "expired" || connection.phase === "failed") {
    return "status-dot status-dot--warn";
  }
  return "status-dot";
}

function connectionTooltipText(connection: ConnectionState, statusText: string, text: LocalizedText): string {
  const session = connection.phase === "connecting" ? null : connection.session;
  if (!session || (connection.phase !== "ready" && connection.phase !== "unsupported")) {
    return `${statusText} · ${text("Click to configure StarRocks connection", "点击打开 StarRocks 连接配置")}`;
  }
  const tlsLabel = session.tls_mode === "verify_full"
    ? text("TLS verify-full", "TLS 完整校验证书")
    : text("Plaintext connection", "明文连接");
  const tz = session.time_zone?.effective_time_zone;
  return [
    statusText,
    tlsLabel,
    tz ? `${text("Time zone", "时区")} ${tz}` : null,
  ].filter(Boolean).join(" · ");
}

/**
 * Single-row merged top bar (boards/00-shell.html, spec §3 & §5.6):
 * - >= 1024px: shows Overview, Queries, Doctor, Alerts, Loads inline (1280px never wraps; < 1440px collapses secondary text).
 * - 768-1023px: shows 3 items (Overview, Queries, Doctor) + "More" dropdown menu holding Alerts and Loads with badges.
 */
export function TopBar(props: {
  workspace: TopBarWorkspace;
  onSelectWorkspace(workspace: Exclude<TopBarWorkspace, "Settings">): void;
  firstRun: boolean;
  hasDoctorReport: boolean;
  openAlerts: number;
  pausedLoadCount: number;
  historyStatus: HistoryStatusResponse | null;
  onOpenCollection(): void;
  connection: ConnectionState;
  connected: boolean;
  dialogOpen: boolean;
  onOpenConnection(): void;
  connectionTriggerRef?: RefObject<HTMLButtonElement | null>;
  onImport(): void;
  onOpenSettings(): void;
  demoMode?: boolean;
}): JSX.Element {
  const {
    workspace,
    onSelectWorkspace,
    firstRun,
    hasDoctorReport,
    openAlerts,
    pausedLoadCount,
    historyStatus,
    onOpenCollection,
    connection,
    connected,
    dialogOpen,
    onOpenConnection,
    connectionTriggerRef,
    onImport,
    onOpenSettings,
    demoMode = false,
  } = props;
  const { text } = useLocale();
  const statusText = localizedSessionStatus(connection, text);
  const connTip = connectionTooltipText(connection, statusText, text);
  const moreActive = workspace === "Alerts" || workspace === "Loads";
  const moreCount = (openAlerts > 0 ? openAlerts : 0) + (pausedLoadCount > 0 ? pausedLoadCount : 0);

  const countFor = (item: Exclude<TopBarWorkspace, "Settings">): number | null => {
    if (item === "Alerts" && openAlerts > 0) return openAlerts;
    if (item === "Loads" && pausedLoadCount > 0) return pausedLoadCount;
    return null;
  };

  const badgeAriaLabel = (item: "Alerts" | "Loads", count: number): string =>
    item === "Alerts"
      ? text(`${count} new alerts`, `${count} 条新告警`)
      : text(`${count} paused jobs`, `${count} 个作业已暂停`);

  return (
    <header className="app-header">
      <div className="brand-lockup">
        <span aria-hidden="true" className="brand-mark">
          <svg fill="none" height="14" viewBox="0 0 18 18" width="14">
            <path d="M4 5.5h10M4 9h10M4 12.5h7" stroke="currentColor" strokeLinecap="round" strokeWidth="1.8" />
          </svg>
        </span>
        <strong>SR Doctor</strong>
      </div>

      <nav aria-label={text("Workspaces", "工作区")} className="top-nav workspace-nav">
        {PRIMARY_WORKSPACES.map((item) => {
          const current = workspace === item;
          const disabled = item === "Doctor" && firstRun && !hasDoctorReport;
          return (
            <button
              aria-current={current ? "page" : undefined}
              className={current ? "nav-link is-active" : "nav-link"}
              disabled={disabled}
              key={item}
              onClick={() => onSelectWorkspace(item)}
              type="button"
            >
              <span>{workspaceLabel(item, text)}</span>
            </button>
          );
        })}

        {OVERFLOW_WORKSPACES.map((item) => {
          const current = workspace === item;
          const count = countFor(item);
          return (
            <button
              aria-current={current ? "page" : undefined}
              className={current ? "nav-link nav-item--collapse-md is-active" : "nav-link nav-item--collapse-md"}
              key={item}
              onClick={() => onSelectWorkspace(item)}
              type="button"
            >
              <span>{workspaceLabel(item, text)}</span>
              {count !== null ? (
                <span
                  aria-label={badgeAriaLabel(item, count)}
                  className={item === "Alerts" ? "nav-count nav-count--alert" : "nav-count"}
                >
                  {count}
                </span>
              ) : null}
            </button>
          );
        })}

        <div className="nav-more-wrap">
          <DropdownMenu.Root>
            <DropdownMenu.Trigger
              aria-label={text("More workspaces", "更多工作区")}
              className={moreActive ? "nav-link is-active" : "nav-link"}
            >
              <span>{text("More", "更多")}</span>
              {moreCount > 0 ? (
                <span className={openAlerts > 0 ? "nav-count nav-count--alert" : "nav-count"}>
                  {moreCount}
                </span>
              ) : null}
              <Icon name="chevronDown" size={12} />
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DropdownMenu.Content align="start" className="nav-more-menu" collisionPadding={12} sideOffset={6}>
                {OVERFLOW_WORKSPACES.map((item) => {
                  const current = workspace === item;
                  const count = countFor(item);
                  return (
                    <DropdownMenu.Item
                      aria-current={current ? "page" : undefined}
                      className={current ? "nav-link is-active" : "nav-link"}
                      key={item}
                      onSelect={() => onSelectWorkspace(item)}
                    >
                      <span>{workspaceLabel(item, text)}</span>
                      {count !== null ? (
                        <span className={item === "Alerts" ? "nav-count nav-count--alert" : "nav-count"}>
                          {count}
                        </span>
                      ) : null}
                    </DropdownMenu.Item>
                  );
                })}
              </DropdownMenu.Content>
            </DropdownMenu.Portal>
          </DropdownMenu.Root>
        </div>
      </nav>

      <div className="header-right">
        {demoMode ? <BadgeStamp kind="demo" /> : null}

        <CollectionCapsule onOpen={onOpenCollection} status={historyStatus} />

        <Tip content={connTip} side="bottom">
          <button
            aria-describedby="connection-pill-status"
            aria-expanded={dialogOpen}
            aria-haspopup="dialog"
            aria-label={connected ? text("Manage connection", "管理连接") : text("Connect", "连接")}
            className="conn-pill"
            onClick={onOpenConnection}
            ref={connectionTriggerRef}
            type="button"
          >
            <span aria-hidden="true" className={connectionDotClass(connection)} />
            <span id="connection-pill-status"><HeaderStatusText value={statusText} /></span>
          </button>
        </Tip>

        {firstRun ? null : (
          <button
            className="btn btn-secondary btn-import-top"
            onClick={onImport}
            title={text("Import profile", "导入 Profile")}
            type="button"
          >
            <Icon name="upload" size={15} />
            <span className="hide-lt-1440">{text("Import profile", "导入 Profile")}</span>
          </button>
        )}

        <Tip content={text("Settings", "设置")} side="bottom">
          <button
            aria-current={workspace === "Settings" ? "page" : undefined}
            aria-label={text("Settings", "设置")}
            className="icon-btn"
            onClick={onOpenSettings}
            type="button"
          >
            <Icon name="settings" size={17} />
          </button>
        </Tip>
      </div>
    </header>
  );
}
