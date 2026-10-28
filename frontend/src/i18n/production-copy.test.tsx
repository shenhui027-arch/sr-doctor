import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { ConnectionDialog } from "../features/connection/ConnectionDialog";
import { initialConnectionState } from "../features/connection/connection-state";
import { ImportPanel } from "../features/import/ImportPanel";
import { LocaleProvider } from "./locale";

describe("production bilingual copy", () => {
  it("renders the read-only connection boundary in Chinese without translating protocol terms", () => {
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><ConnectionDialog dispatch={vi.fn()} onClose={vi.fn()} open state={initialConnectionState} /></LocaleProvider>);

    expect(html).toContain("只读连接");
    expect(html).toContain("Runtime Top 需显式开启");
    expect(html).toContain("超时固定为 10 秒并丢弃结果行");
    expect(html).toContain("DDL、DML、EXPLAIN ANALYZE");
    expect(html).toContain("TLS verify-full");
    // Plaintext is an explicit, labelled choice for any host.
    expect(html).toContain("口令与 SQL 不加密传输");
    expect(html).toContain('role="dialog"');
    expect(html).toContain('aria-describedby="connection-boundary"');
    expect(html).toContain('role="status"');
  });

  it("renders the memory-only import boundary in Chinese", () => {
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><ImportPanel onReport={vi.fn()} /></LocaleProvider>);

    expect(html).toContain("离线 · 无需连接");
    expect(html).toContain("原文只在本机进程内分析，不写入日志或遥测。");
    expect(html).not.toContain("localStorage");
    expect(html).toContain('aria-busy="false"');
    expect(html).toContain('aria-describedby="import-profile-status"');
  });
});
