import type { AlertsView, DoctorMode, QueryMode, Workspace } from "./App";
import type { SettingsSection } from "./features/settings/SettingsWorkspace";

export type OverviewTab = "performance" | "trends";
export type PrimarySettingsSection = "history" | "notify" | "connection";

export type AppRouteState = {
  workspace: Workspace;
  overviewTab?: OverviewTab;
  queryMode?: QueryMode | null;
  doctorMode?: DoctorMode;
  alertsView?: AlertsView;
  settingsSection?: PrimarySettingsSection;
};

export type AppRouteSnapshot = {
  workspace: Workspace;
  overviewTab: OverviewTab;
  queryMode: QueryMode | null;
  doctorMode: DoctorMode;
  alertsView: AlertsView;
  settingsSection: SettingsSection;
};

const QUERY_MODES: ReadonlySet<QueryMode> = new Set(["audit", "profiles", "running", "sql"]);

function normalizeSettingsSection(section: SettingsSection): PrimarySettingsSection {
  if (section === "deliveries") return "notify";
  if (section === "language") return "connection";
  return section;
}

/**
 * Parses a static workspace/subview hash into route state.
 * Rejects unknown paths and never reads query IDs, SQL, or connection identifiers from the URL.
 */
export function parseAppRouteHash(rawHash: string): AppRouteState | null {
  if (!rawHash || rawHash === "#") {
    return { workspace: "Queries", queryMode: null };
  }
  if (!rawHash.startsWith("#")) return null;
  const trimmed = rawHash.slice(1).replace(/^\/+/, "");
  if (!trimmed) return { workspace: "Queries", queryMode: null };
  const parts = trimmed.split("/");
  if (parts.length > 2) return null;
  const [root, sub] = parts;

  switch (root) {
    case "overview":
      if (sub === undefined) return { workspace: "Overview", overviewTab: "performance" };
      if (sub === "trends") return { workspace: "Overview", overviewTab: "trends" };
      return null;
    case "queries":
      if (sub === undefined) return { workspace: "Queries", queryMode: null };
      if (QUERY_MODES.has(sub as QueryMode)) {
        return { workspace: "Queries", queryMode: sub as QueryMode };
      }
      return null;
    case "doctor":
      if (sub === undefined) return { workspace: "Doctor", doctorMode: "diagnosis" };
      if (sub === "compare") return { workspace: "Doctor", doctorMode: "compare" };
      return null;
    case "alerts":
      if (sub === undefined) return { workspace: "Alerts", alertsView: "alerts" };
      if (sub === "governance") return { workspace: "Alerts", alertsView: "governance" };
      return null;
    case "loads":
      if (sub === undefined) return { workspace: "Loads" };
      return null;
    case "settings":
      if (sub === undefined || sub === "notify" || sub === "deliveries") {
        return { workspace: "Settings", settingsSection: "notify" };
      }
      if (sub === "history") return { workspace: "Settings", settingsSection: "history" };
      if (sub === "connection" || sub === "language") {
        return { workspace: "Settings", settingsSection: "connection" };
      }
      return null;
    default:
      return null;
  }
}

/** Formats the active workspace and subview into a whitelist URL hash (or "" for the default landing state). */
export function formatAppRouteHash(state: AppRouteSnapshot): string {
  switch (state.workspace) {
    case "Overview":
      return state.overviewTab === "trends" ? "#overview/trends" : "#overview";
    case "Queries":
      return state.queryMode ? `#queries/${state.queryMode}` : "";
    case "Doctor":
      return state.doctorMode === "compare" ? "#doctor/compare" : "#doctor";
    case "Alerts":
      return state.alertsView === "governance" ? "#alerts/governance" : "#alerts";
    case "Loads":
      return "#loads";
    case "Settings": {
      const normalized = normalizeSettingsSection(state.settingsSection);
      return normalized === "notify" ? "#settings" : `#settings/${normalized}`;
    }
  }
}
