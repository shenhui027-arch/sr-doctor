import { describe, expect, it } from "vitest";
import { formatAppRouteHash, parseAppRouteHash, type AppRouteSnapshot } from "./app-route-hash";

const baseSnapshot: AppRouteSnapshot = {
  workspace: "Queries",
  overviewTab: "performance",
  queryMode: null,
  doctorMode: "diagnosis",
  alertsView: "alerts",
  settingsSection: "notify",
};

describe("app-route-hash", () => {
  it("formats and parses every workspace and subview round-trip without leaking identifiers", () => {
    const cases: [AppRouteSnapshot, string][] = [
      [{ ...baseSnapshot }, ""],
      [{ ...baseSnapshot, queryMode: "audit" }, "#queries/audit"],
      [{ ...baseSnapshot, queryMode: "profiles" }, "#queries/profiles"],
      [{ ...baseSnapshot, queryMode: "running" }, "#queries/running"],
      [{ ...baseSnapshot, queryMode: "sql" }, "#queries/sql"],
      [{ ...baseSnapshot, workspace: "Overview", overviewTab: "performance" }, "#overview"],
      [{ ...baseSnapshot, workspace: "Overview", overviewTab: "trends" }, "#overview/trends"],
      [{ ...baseSnapshot, workspace: "Doctor", doctorMode: "diagnosis" }, "#doctor"],
      [{ ...baseSnapshot, workspace: "Doctor", doctorMode: "compare" }, "#doctor/compare"],
      [{ ...baseSnapshot, workspace: "Alerts", alertsView: "alerts" }, "#alerts"],
      [{ ...baseSnapshot, workspace: "Alerts", alertsView: "governance" }, "#alerts/governance"],
      [{ ...baseSnapshot, workspace: "Loads" }, "#loads"],
      [{ ...baseSnapshot, workspace: "Settings", settingsSection: "notify" }, "#settings"],
      [{ ...baseSnapshot, workspace: "Settings", settingsSection: "deliveries" }, "#settings"],
      [{ ...baseSnapshot, workspace: "Settings", settingsSection: "history" }, "#settings/history"],
      [{ ...baseSnapshot, workspace: "Settings", settingsSection: "connection" }, "#settings/connection"],
      [{ ...baseSnapshot, workspace: "Settings", settingsSection: "language" }, "#settings/connection"],
    ];

    for (const [snapshot, expectedHash] of cases) {
      const formatted = formatAppRouteHash(snapshot);
      expect(formatted).toBe(expectedHash);
      const parsed = parseAppRouteHash(formatted);
      expect(parsed).not.toBeNull();
      expect(parsed?.workspace).toBe(snapshot.workspace);
    }
  });

  it("parses #queries explicitly and normalizes legacy settings aliases", () => {
    expect(parseAppRouteHash("#queries")).toStrictEqual({ workspace: "Queries", queryMode: null });
    expect(parseAppRouteHash("#settings/notify")).toStrictEqual({ workspace: "Settings", settingsSection: "notify" });
    expect(parseAppRouteHash("#settings/deliveries")).toStrictEqual({ workspace: "Settings", settingsSection: "notify" });
    expect(parseAppRouteHash("#settings/language")).toStrictEqual({ workspace: "Settings", settingsSection: "connection" });
  });

  it("rejects unknown, multi-segment, or dynamic/injected hashes", () => {
    for (const invalid of [
      "overview",
      "#unknown",
      "#doctor/compare/extra",
      "#doctor/qid-1234",
      "#queries/audit?user=root",
      "#overview/SELECT%201",
      "#settings/<script>",
    ]) {
      expect(parseAppRouteHash(invalid)).toBeNull();
    }
  });
});
