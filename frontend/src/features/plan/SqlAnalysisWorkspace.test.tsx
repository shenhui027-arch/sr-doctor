import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { sr3514ConnectedContractGolden } from "../../../../contracts/generated/connected-contract-fixtures";
import { LocaleProvider } from "../../i18n/locale";
import { planFailureLabel, SqlAnalysisWorkspace, type SqlAnalysisWorkspaceProps } from "./SqlAnalysisWorkspace";

function props(overrides: Partial<SqlAnalysisWorkspaceProps> = {}): SqlAnalysisWorkspaceProps {
  return {
    onBackToDraft: vi.fn(),
    onConfirmExecution: vi.fn(),
    onPlanModeChange: vi.fn(),
    onRequestConfirmation: vi.fn(),
    onSqlChange: vi.fn(),
    onStopWaiting: vi.fn(),
    onViewPlan: vi.fn(),
    planMode: "costs",
    sql: "SELECT 1",
    stage: "draft",
    connection: { connectionId: "c", tlsMode: "docker_loopback_plaintext", detectedVersion: "3.5.14" },
    ...overrides,
  };
}

const render = (overrides: Partial<SqlAnalysisWorkspaceProps> = {}) => renderToStaticMarkup(<LocaleProvider initialLocale="zh"><SqlAnalysisWorkspace {...props(overrides)} /></LocaleProvider>);

describe("SQL workspace", () => {
  it("offers the readonly plan first and one standard Profile execution, never native EXPLAIN ANALYZE", () => {
    const html = render();
    expect(html).toContain("1 · 查看计划（不执行）");
    expect(html).toContain("2 · 执行一次并诊断");
    expect(html).toContain("检查并执行…");
    expect(html).not.toContain("EXPLAIN ANALYZE");
    expect(html).not.toContain("原生");
  });
  it("confirms the exact SQL, timeout and single submission before execution", () => {
    const html = render({ stage: "confirm", sql: "\nSELECT 48 -- keep\n" });
    expect(html).toContain("执行一次并采集 Profile");
    expect(html).toContain("超时 10 秒");
    expect(html).toContain("SELECT 48 -- keep");
    expect(html).toContain(">执行一次<");
  });
  it.each(["pending", "unknown"] as const)("locks the SQL and hides execute while %s", (stage) => {
    const html = render({ stage });
    expect(html).not.toContain("检查并执行…");
    expect(html).toContain("disabled");
    expect(html).toContain(stage === "pending" ? "停止等待" : "执行结果未知");
  });
  it("shows the plan summary only for the mode that produced it", () => {
    const snapshot = sr3514ConnectedContractGolden.explain_response.snapshot;
    expect(render({ planSnapshot: snapshot, planResultMode: "costs" })).toContain("打开计划");
    expect(render({ planSnapshot: snapshot, planResultMode: "logical" })).not.toContain("打开计划");
  });
  it("maps plan failures to actionable copy", () => {
    const text = (_en: string, zh: string) => zh;
    expect(planFailureLabel("connection_expired", text)).toContain("重新连接");
    expect(planFailureLabel("remote_timeout", text)).toContain("超时");
  });
});
