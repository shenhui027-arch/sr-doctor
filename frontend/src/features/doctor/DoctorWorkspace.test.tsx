import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { sr3514ExecutionGraph } from "../../../../contracts/generated/execution-graph-fixture";
import { handleDialogCloseAutoFocus, syncDialogOpenLayout } from "../../design";
import { LocaleProvider } from "../../i18n/locale";
import { doctorReport } from "../../visual/demo-doctor";
import {
  closeFindingState,
  createSelectionState,
  selectFindingState,
  selectSourceRefState,
  showFindingNodeTransition,
} from "../selection/selection-store";
import { ProvenanceAndPlanCheck } from "./DoctorDetails";
import { DoctorWorkspace } from "./DoctorWorkspace";

describe("DoctorWorkspace v1.1 (04-doctor)", () => {
  it("renders the 7-cell metric strip with MetricName (no brackets), single multi-tab left card, and single findings list card without duplicate compare button", () => {
    const html = renderToStaticMarkup(
      <LocaleProvider initialLocale="zh">
        <DoctorWorkspace onOpenCompare={() => {}} onReport={() => {}} report={sr3514ExecutionGraph} />
      </LocaleProvider>,
    );

    expect(html).toContain('class="kpi-grid kpi-grid--7 doctor-summary"');
    expect(html.match(/class="card card--kpi"/g)).toHaveLength(7);
    expect(html).toContain('<span class="metric-name"><span>总耗时</span><span class="metric-sub" title="QueryTotalTime">QueryTotalTime</span></span>');
    expect(html).toContain('<span class="metric-name"><span>规划耗时</span><span class="metric-sub" title="QueryPlanningTime">QueryPlanningTime</span></span>');
    expect(html).toContain('<span class="metric-name"><span>返回行数</span><span class="metric-sub" title="ResultRows">ResultRows</span></span>');
    expect(html).not.toContain("(QueryTotalTime)");

    expect(html).toContain('class="card card--table doctor-main-card"');
    expect(html).toContain('class="doctor-findings-column"');
    expect(html).toContain('class="card card--list findings-panel"');
    expect(html).not.toContain("card-stack-2");
    expect(html).not.toContain("finding-inspector");
    expect(html).not.toContain('role="dialog"');

    // `设为基线并对比` lives only in ContextBar, never duplicated inside DoctorWorkspace.
    expect(html).not.toContain("设为基线并对比");
    expect(html).not.toContain("前后对比");
  });

  it("renders a single right-column list card with no inspector card on load, and renders the problem statement and evidence section when the drawer is open", () => {
    const demo = doctorReport();
    const closedHtml = renderToStaticMarkup(
      <LocaleProvider initialLocale="zh">
        <DoctorWorkspace onOpenCompare={() => {}} onReport={() => {}} report={demo} />
      </LocaleProvider>,
    );

    expect(closedHtml).toContain('<aside aria-label="发现" class="doctor-findings-column">');
    expect(closedHtml.match(/class="card card--list findings-panel"/g)).toHaveLength(1);
    expect(closedHtml).not.toContain("card-stack-2");
    expect(closedHtml).not.toContain("finding-inspector");
    expect(closedHtml).not.toContain('role="dialog"');
    expect(closedHtml).not.toContain("问题陈述");
    expect(closedHtml).toContain('aria-label="高 · Hash Join 探测侧数据量大，耗时集中 · HASH_JOIN · Plan 4"');
    expect(closedHtml).toContain("exec-node-finding-btn");

    const openHtml = renderToStaticMarkup(
      <LocaleProvider initialLocale="zh">
        <DoctorWorkspace
          initialFindingId={demo.findings[0].finding_id}
          onOpenCompare={() => {}}
          onReport={() => {}}
          report={demo}
        />
      </LocaleProvider>,
    );

    expect(openHtml).not.toContain("finding-inspector");
    expect(openHtml).toContain('role="dialog"');
    expect(openHtml).toContain("modal-card modal-card--sheet modal-card--wide");
    expect(openHtml).toContain("Hash Join 探测侧数据量大，耗时集中");
    expect(openHtml).toContain("问题陈述");
    expect(openHtml).toContain("Hash Join 探测侧数据量大，耗时集中；根因仍待验证。");
    expect(openHtml).toContain("<h3>证据</h3>");
    expect(openHtml).toContain('<span class="metric-name"><span><span class="inspector-metric">算子活跃耗时</span></span><span class="metric-sub" title="OperatorTotalTime">OperatorTotalTime</span></span>');
    expect(openHtml).toContain("1,102,223,445");
    expect(openHtml).toContain("查看原文 L214");
    expect(openHtml).toContain("在图中定位");
    expect(openHtml).not.toContain("设为基线并对比");
    expect(openHtml).not.toContain("前后对比");
  });

  it("shows only existing plan check facts linked to the query and never offers a run-new-plan-check button", () => {
    const unlinked = renderToStaticMarkup(
      <LocaleProvider initialLocale="zh">
        <ProvenanceAndPlanCheck report={sr3514ExecutionGraph} />
      </LocaleProvider>,
    );

    expect(unlinked).toContain("只读计划检查 &amp; Plan Advisor");
    expect(unlinked).toContain("计划估算不与实测混算");
    expect(unlinked).toContain("当前 Profile 未关联已有计划检查结果");
    expect(unlinked).not.toMatch(/EXPLAIN\s+COSTS/);
    expect(unlinked).not.toContain("运行计划检查");

    const linked = renderToStaticMarkup(
      <LocaleProvider initialLocale="zh">
        <ProvenanceAndPlanCheck report={doctorReport()} />
      </LocaleProvider>,
    );

    expect(linked).toContain("只读计划检查 &amp; Plan Advisor");
    expect(linked).toContain("Plan 0 · OLAP_SCAN");
    expect(linked).toContain("demo.orders");
    expect(linked).not.toMatch(/EXPLAIN\s+COSTS/);
    expect(linked).not.toContain("运行计划检查");
  });

  it("returns focus to the finding trigger on cancel close, and suppresses focus return when closing via 在图中定位 or 查看原文", () => {
    const demo = doctorReport();
    const finding = demo.findings[0];
    const opener = { isConnected: true, focus: vi.fn() } as unknown as HTMLElement;
    const wasOpenRef = { current: false };
    const returnFocusRef: { current: HTMLElement | null } = { current: null };

    // Cancel close (Esc, close button, overlay): restoreFindingFocus stays true, so opener receives focus.
    const openedForCancel = selectFindingState(createSelectionState(), finding.finding_id, finding.subject_ref);
    syncDialogOpenLayout(openedForCancel.findingOpen, wasOpenRef, returnFocusRef, opener);
    const cancelled = closeFindingState(openedForCancel);
    syncDialogOpenLayout(cancelled.findingOpen, wasOpenRef, returnFocusRef, null);
    const cancelEvent = { preventDefault: vi.fn() };
    handleDialogCloseAutoFocus(cancelEvent, returnFocusRef, cancelled.restoreFindingFocus);
    expect(opener.focus).toHaveBeenCalledTimes(1);

    // Navigating close via "在图中定位": restoreFindingFocus is false, so focus is not pulled back to opener.
    opener.focus = vi.fn();
    const openedForGraph = selectFindingState(cancelled, finding.finding_id, finding.subject_ref);
    syncDialogOpenLayout(openedForGraph.findingOpen, wasOpenRef, returnFocusRef, opener);
    const locatedInGraph = showFindingNodeTransition(openedForGraph, "4");
    syncDialogOpenLayout(locatedInGraph.findingOpen, wasOpenRef, returnFocusRef, null);
    const graphCloseEvent = { preventDefault: vi.fn() };
    handleDialogCloseAutoFocus(graphCloseEvent, returnFocusRef, locatedInGraph.restoreFindingFocus);
    expect(graphCloseEvent.preventDefault).toHaveBeenCalledTimes(1);
    expect(opener.focus).not.toHaveBeenCalled();

    // Navigating close via "查看原文 L214": restoreFindingFocus is false, so focus stays on the source row.
    const openedForSource = selectFindingState(locatedInGraph, finding.finding_id, finding.subject_ref);
    syncDialogOpenLayout(openedForSource.findingOpen, wasOpenRef, returnFocusRef, opener);
    const viewedSource = selectSourceRefState(openedForSource, "text:214");
    syncDialogOpenLayout(viewedSource.findingOpen, wasOpenRef, returnFocusRef, null);
    const sourceCloseEvent = { preventDefault: vi.fn() };
    handleDialogCloseAutoFocus(sourceCloseEvent, returnFocusRef, viewedSource.restoreFindingFocus);
    expect(sourceCloseEvent.preventDefault).toHaveBeenCalledTimes(1);
    expect(opener.focus).not.toHaveBeenCalled();
  });
});
