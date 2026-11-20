import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { HistoryStatusResponse } from "../../../../contracts/generated/doctor-contracts";
import { LocaleProvider as LocaleProviderWithChildren } from "../../i18n/locale";
import type { ComponentProps, FunctionComponent, ReactNode } from "react";

// createElement(LocaleProvider, props, child) passes the child as the third
// argument; this alias lets TypeScript accept that form in tests.
const LocaleProvider = LocaleProviderWithChildren as FunctionComponent<
  Omit<ComponentProps<typeof LocaleProviderWithChildren>, "children"> & { children?: ReactNode }
>;
import { ConnectionForm, saveUnavailableText } from "../connection/ConnectionForm";
import { initialConnectionState } from "../connection/connection-state";
import { SavedConnectionCard } from "./SavedConnectionCard";
import { SettingsWorkspace } from "./SettingsWorkspace";

const en = (english: string) => english;
const zh = (_english: string, chinese: string) => chinese;

const sampleHistoryStatus: HistoryStatusResponse = {
  store: { state: "available", retention_days: 30, file_bytes: "2516582", templates: "12", buckets: "40" },
  target: "127.0.0.1:9030",
  collection: { state: "running", next_run_seconds: 1_800_000_300 },
  watermark_seconds: 1_800_000_000,
  budget: { period_seconds: 300, slice_seconds: 300, lag_seconds: 120, slice_row_limit: 1000, query_timeout_seconds: 15, catch_up_seconds: 86_400, production_budget_measured: false },
  coverage: [],
  runs: [],
  remote_collection_enabled: false,
  open_alerts: "0",
  load_revision: "1",
  limitations: [],
};

describe("saved connection and SettingsWorkspace (boards/08-settings)", () => {
  it("explains every reason saving is off in both languages", () => {
    for (const reason of ["history_unavailable", "key_unavailable", "default_admin_password"] as const) {
      expect(saveUnavailableText(reason, en)).toMatch(/\w/);
      expect(saveUnavailableText(reason, zh)).not.toBe(saveUnavailableText(reason, en));
    }
    expect(saveUnavailableText("default_admin_password", zh)).toContain("DOCTOR_ADMIN_PASSWORD");
  });

  it("offers saving as an unticked choice that stays disabled until the server answers", () => {
    const html = renderToStaticMarkup(
      createElement(LocaleProvider, { initialLocale: "en" }, createElement(ConnectionForm, { dispatch: () => undefined, idPrefix: "t", state: initialConnectionState })),
    );
    expect(html).toContain("Save the password encrypted on the SR Doctor server and keep collecting");
    // Disabled and unticked: no `checked` attribute on the checkbox itself.
    expect(html).toMatch(/<input disabled="" type="checkbox"\/>/);
  });

  it("starts as a loading card with no forget action before the view arrives", () => {
    const html = renderToStaticMarkup(createElement(LocaleProvider, { initialLocale: "en" }, createElement(SavedConnectionCard)));
    expect(html).toContain("Saved connection");
    expect(html).not.toContain("Forget saved connection");
  });

  it("renders three top-bar sections, merges language into Connection & preferences, and mounts CollectionPanel directly", () => {
    const connHtml = renderToStaticMarkup(
      createElement(
        LocaleProvider,
        { initialLocale: "zh" },
        createElement(SettingsWorkspace, {
          connectionStatus: "已连接 · 3.5.14 · 只读",
          history: { status: sampleHistoryStatus, error: null, refresh: () => undefined },
          locale: "zh",
          onLocale: () => undefined,
          onOpenConnection: () => undefined,
          section: "connection",
          tlsMode: "plaintext",
        }),
      ),
    );
    expect(connHtml).toContain("历史库与采集");
    expect(connHtml).toContain("告警通知");
    expect(connHtml).toContain("连接与偏好");
    expect(connHtml).toContain("界面语言与安全边界");
    expect(connHtml).toContain("明文传输提醒");
    expect(connHtml).toContain("规划中 · Planned");
    expect(connHtml).not.toContain("card-deck");

    const histHtml = renderToStaticMarkup(
      createElement(
        LocaleProvider,
        { initialLocale: "zh" },
        createElement(SettingsWorkspace, {
          connectionStatus: "已连接 · 3.5.14 · 只读",
          history: { status: sampleHistoryStatus, error: null, refresh: () => undefined },
          locale: "zh",
          onLocale: () => undefined,
          onOpenConnection: () => undefined,
          section: "history",
        }),
      ),
    );
    expect(histHtml).toContain("增量采集");
    expect(histHtml).toContain("24 小时覆盖与最近读取");
    expect(histHtml).toContain("本机历史库");
    expect(histHtml).toContain(">清空历史…</button>");
    expect(histHtml).not.toContain("history-tabs");
    expect(histHtml).not.toContain("card-deck");
  });
});
