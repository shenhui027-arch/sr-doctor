import * as Tabs from "@radix-ui/react-tabs";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { LocaleProvider } from "../i18n/locale";
import {
  AnimatedFigure,
  BadgeStamp,
  Button,
  Card,
  Dialog,
  EvidenceMeter,
  handleDialogCloseAutoFocus,
  InfoTip,
  KpiCard,
  MetricName,
  PageGrid,
  parseAnimatedFigure,
  resolveNextSegTabValue,
  SegTabs,
  shouldAnimateFigureTransition,
  StateStamp,
  syncDialogOpenLayout,
  TooltipProvider,
  type HonestState,
} from "./index";

describe("v1.1 design primitives", () => {
  it("renders Card with fixed header, single scroll body, optional footer, and responsive GridSpan classes", () => {
    const html = renderToStaticMarkup(
      <LocaleProvider initialLocale="zh">
        <PageGrid aria-label="测试栅格">
          <Card
            actions={<Button size="sm" variant="secondary">导出</Button>}
            footer={<span>返回 100 / 窗口 1,284 条</span>}
            span={{ xl: 6, lg: 8, md: 8, sm: 4 }}
            title="算子排行"
            titleExtra={<InfoTip label="口径说明">按累计耗时降序</InfoTip>}
            variant="table"
          >
            <div>表格主体</div>
          </Card>
          <Card scroll={false} span={{ xl: 6 }} title="非滚动卡片">
            <div>画布</div>
          </Card>
        </PageGrid>
      </LocaleProvider>,
    );

    expect(html).toContain('class="page-grid"');
    expect(html).toContain('role="region"');
    expect(html).toContain("card card--table span-lg-8 span-xl-6 span-md-8 span-sm-4");
    expect(html).toContain('class="card-hd"');
    expect(html).toContain('class="card-bd card-bd--flush"');
    expect(html).toContain('class="card-ft"');
    expect(html).toContain("返回 100 / 窗口 1,284 条");
    expect(html).toContain("card card--default span-lg-6 span-xl-6");
    expect(html).toContain('class="card-bd card-bd--no-scroll"');
  });

  it("renders SegTabs on Radix Tabs with active pill, counts, disabled items, and keyboard navigation", () => {
    const items = [
      { value: "families", label: "同类 SQL", count: 48 },
      { value: "similar", label: "相似 SQL", count: 12, disabled: true },
      { value: "occurrences", label: "单次执行" },
    ] as const;

    const html = renderToStaticMarkup(
      <SegTabs
        aria-label="查询视图切换"
        items={items}
        onValueChange={() => undefined}
        size="sm"
        value={"families" as (typeof items)[number]["value"]}
      />,
    );

    expect(html).toContain('role="tablist"');
    expect(html).toContain("seg-tabs has-pill seg-tabs--sm");
    expect(html).toContain('aria-selected="true"');
    expect(html).toContain('class="seg-tab-pill"');
    expect(html).toContain('<span class="seg-count">48</span>');
    expect(html).toContain("disabled");

    // Keyboard navigation skips disabled items and loops cleanly at boundaries.
    expect(resolveNextSegTabValue(items, "families", "ArrowRight")).toBe("occurrences");
    expect(resolveNextSegTabValue(items, "occurrences", "ArrowRight")).toBe("families");
    expect(resolveNextSegTabValue(items, "families", "ArrowLeft")).toBe("occurrences");
    expect(resolveNextSegTabValue(items, "occurrences", "Home")).toBe("families");
    expect(resolveNextSegTabValue(items, "families", "End")).toBe("occurrences");

    // Supports mounted Radix Tabs.Content panels when passed as children.
    const withPanels = renderToStaticMarkup(
      <SegTabs aria-label="带面板标签" items={items} onValueChange={() => undefined} value={"families" as (typeof items)[number]["value"]}>
        <Tabs.Content forceMount value="families">面板一</Tabs.Content>
        <Tabs.Content forceMount value="occurrences">面板二</Tabs.Content>
      </SegTabs>,
    );
    expect(withPanels).toContain('role="tabpanel"');
    expect(withPanels).toContain("面板一");
    expect(withPanels).toContain("面板二");
  });

  it("renders Button variants, sizes, and icons with single primary CTA styling", () => {
    const html = renderToStaticMarkup(
      <div>
        <Button icon="play" variant="primary">拉取并诊断</Button>
        <Button size="sm" variant="secondary">次要操作</Button>
        <Button variant="warning">执行一次 SQL</Button>
        <Button variant="danger-outline">清空历史…</Button>
        <Button disabled variant="danger">确认清空</Button>
        <Button variant="ghost">返回</Button>
      </div>,
    );

    expect(html).toContain('class="btn btn-primary"');
    expect(html).toContain('class="btn btn-secondary btn--sm"');
    expect(html).toContain('class="btn btn-warning"');
    expect(html).toContain('class="btn btn-danger-outline"');
    expect(html).toContain('class="btn btn-danger"');
    expect(html).toContain('class="btn btn-ghost"');
    expect(html).toContain('type="button"');
  });

  it("renders distinct symbols and bilingual copy for every HonestState and BadgeStamp, never turning missing into 0", () => {
    const states: readonly HonestState[] = [
      "missing",
      "invalid",
      "partial",
      "overflow",
      "unknown_version",
      "not_comparable",
      "unscored",
    ];

    const zhHtml = renderToStaticMarkup(
      <LocaleProvider initialLocale="zh">
        <div>
          {states.map((state) => <StateStamp key={state} state={state} />)}
          <StateStamp detail="Profile 未打印 ResultRows" state="missing" />
          <BadgeStamp kind="planned" />
          <BadgeStamp kind="demo" />
          <BadgeStamp kind="official_digest" />
          <BadgeStamp kind="unofficial_fingerprint" />
        </div>
      </LocaleProvider>,
    );

    expect(zhHtml).toContain("∅ 缺失 · 非 0");
    expect(zhHtml).toContain("! 无效");
    expect(zhHtml).toContain("◐ 部分");
    expect(zhHtml).toContain("↑ 溢出");
    expect(zhHtml).toContain("? 版本未知");
    expect(zhHtml).toContain("≠ 不可比");
    expect(zhHtml).toContain("— 未评分");
    expect(zhHtml).toContain("state-stamp--critical");
    expect(zhHtml).toContain("state-stamp--warn");
    expect(zhHtml).toContain('aria-label="缺失 · 非 0说明"');
    expect(zhHtml).toContain("规划中 · Planned");
    expect(zhHtml).toContain("Demo / Mock cluster");
    expect(zhHtml).toContain("官方 digest");
    expect(zhHtml).toContain("非官方指纹");

    const enHtml = renderToStaticMarkup(
      <LocaleProvider initialLocale="en">
        <div>
          {states.map((state) => <StateStamp key={state} state={state} />)}
          <BadgeStamp kind="unofficial_fingerprint" />
        </div>
      </LocaleProvider>,
    );

    expect(enHtml).toContain("∅ Missing · not 0");
    expect(enHtml).toContain("! Invalid");
    expect(enHtml).toContain("◐ Partial");
    expect(enHtml).toContain("↑ Overflow");
    expect(enHtml).toContain("? Unknown version");
    expect(enHtml).toContain("≠ Not comparable");
    expect(enHtml).toContain("— Unscored");
    expect(enHtml).toContain("Unofficial fingerprint");
  });

  it("renders MetricName on two lines without parentheses and KpiCard with 24x24 InfoTip", () => {
    const html = renderToStaticMarkup(
      <LocaleProvider initialLocale="zh">
        <TooltipProvider>
          <MetricName label="算子累计耗时" official="QueryCumulativeOperatorTime" />
          <KpiCard
            foot="3 条为空未计"
            info="按 queryId 去重统计"
            label="执行失败"
            span={{ lg: 3 }}
            tone="critical"
            value="12"
          />
          <EvidenceMeter detail="缺少返回行数" label="证据 部分" level="partial" />
        </TooltipProvider>
      </LocaleProvider>,
    );

    expect(html).toContain('<span class="metric-name"><span>算子累计耗时</span><span class="metric-sub" title="QueryCumulativeOperatorTime">QueryCumulativeOperatorTime</span></span>');
    expect(html).not.toContain("(QueryCumulativeOperatorTime)");
    expect(html).toContain("card card--kpi span-lg-3");
    expect(html).toContain("kpi-val kpi-val--critical");
    expect(html).toContain('class="info-btn ui-info"');
    expect(html).toContain("3 条为空未计");
    expect(html).toContain('data-level="partial"');
  });

  it("renders Dialog on Radix Dialog with accessible title, description, close button, and danger/wide variants", () => {
    const closedHtml = renderToStaticMarkup(
      <LocaleProvider initialLocale="zh">
        <Dialog onOpenChange={() => undefined} open={false} title="隐藏弹窗">
          <p>不可见内容</p>
        </Dialog>
      </LocaleProvider>,
    );
    expect(closedHtml).toBe("");

    const openHtml = renderToStaticMarkup(
      <LocaleProvider initialLocale="zh">
        <Dialog
          description="不可逆操作：将立即删除本机历史库数据。"
          footer={<Button variant="danger">确认清空历史</Button>}
          onOpenChange={() => undefined}
          open
          title="清空本机历史库确认"
          tone="danger"
          width="lg"
        >
          <p>请确认是否继续。</p>
        </Dialog>
      </LocaleProvider>,
    );

    expect(openHtml).toContain('role="dialog"');
    expect(openHtml).toContain("modal-card modal-card--wide modal-card--danger");
    expect(openHtml).toContain("清空本机历史库确认");
    expect(openHtml).toContain("不可逆操作：将立即删除本机历史库数据。");
    expect(openHtml).toContain('aria-label="关闭弹窗"');
    expect(openHtml).toContain("确认清空历史");
    // Radix links aria-labelledby/-describedby after Title/Description mount
    // (an effect), so static markup can only prove both carry their ids.
    expect(openHtml).toMatch(/<h2 id="[^"]+" class="card-title">/);
    expect(openHtml).toMatch(/<p id="[^"]+" class="modal-desc">/);
  });

  it("renders Dialog with placement=\"right\" as a right-side sheet", () => {
    const sheetHtml = renderToStaticMarkup(
      <LocaleProvider initialLocale="zh">
        <Dialog
          footer={<Button size="sm" variant="secondary">在图中定位</Button>}
          onOpenChange={() => undefined}
          open
          placement="right"
          title="算子时间分布"
          width="lg"
        >
          <p>抽屉主体内容</p>
        </Dialog>
      </LocaleProvider>,
    );

    expect(sheetHtml).toContain('role="dialog"');
    expect(sheetHtml).toContain("modal-card modal-card--sheet modal-card--wide");
    expect(sheetHtml).toContain("算子时间分布");
    expect(sheetHtml).toContain("抽屉主体内容");
    expect(sheetHtml).toContain("在图中定位");
    expect(sheetHtml).toContain('aria-label="关闭弹窗"');
  });

  it("captures Dialog return-focus target in layout effect without StrictMode loss, restores focus by default, and skips restoreFocus=false", () => {
    const opener = { isConnected: true, focus: vi.fn() } as unknown as HTMLElement;
    const dialogInternal = { isConnected: true, focus: vi.fn() } as unknown as HTMLElement;
    const secondOpener = { isConnected: true, focus: vi.fn() } as unknown as HTMLElement;
    const wasOpenRef = { current: false };
    const returnFocusRef: { current: HTMLElement | null } = { current: null };

    // Closed mount does not capture activeElement.
    syncDialogOpenLayout(false, wasOpenRef, returnFocusRef, opener);
    expect(returnFocusRef.current).toBeNull();

    // Opening (`false -> true`) captures the trigger element.
    syncDialogOpenLayout(true, wasOpenRef, returnFocusRef, opener);
    expect(returnFocusRef.current).toBe(opener);

    // StrictMode simulated remount while still open preserves the original opener
    // even if focus has already moved inside the dialog.
    syncDialogOpenLayout(true, wasOpenRef, returnFocusRef, dialogInternal);
    expect(returnFocusRef.current).toBe(opener);

    // Closing (`true -> false`) resets wasOpenRef without wiping returnFocusRef before onCloseAutoFocus runs.
    syncDialogOpenLayout(false, wasOpenRef, returnFocusRef, dialogInternal);
    expect(wasOpenRef.current).toBe(false);
    expect(returnFocusRef.current).toBe(opener);

    // Default Dialog (`restoreFocus = true`) prevents Radix fallback and focuses the opener.
    const cancelCloseEvent = { preventDefault: vi.fn() };
    handleDialogCloseAutoFocus(cancelCloseEvent, returnFocusRef, true);
    expect(cancelCloseEvent.preventDefault).toHaveBeenCalledTimes(1);
    expect(opener.focus).toHaveBeenCalledTimes(1);
    expect(returnFocusRef.current).toBeNull();

    // Re-opening captures the new opener; navigating close (`restoreFocus = false`) prevents
    // Radix from pulling focus back and leaves focus on the navigation target.
    syncDialogOpenLayout(true, wasOpenRef, returnFocusRef, secondOpener);
    syncDialogOpenLayout(false, wasOpenRef, returnFocusRef, dialogInternal);
    const navCloseEvent = { preventDefault: vi.fn() };
    handleDialogCloseAutoFocus(navCloseEvent, returnFocusRef, false);
    expect(navCloseEvent.preventDefault).toHaveBeenCalledTimes(1);
    expect(secondOpener.focus).not.toHaveBeenCalled();
    expect(returnFocusRef.current).toBeNull();
  });

  it("animates AnimatedFigure only when before and after units match, switching directly across unit changes", () => {
    const ms100 = parseAnimatedFigure("100\u00A0ms");
    const ms84 = parseAnimatedFigure("84 ms");
    const sec1 = parseAnimatedFigure("1.2 s");
    const mb512 = parseAnimatedFigure("512 MB");
    const gb1 = parseAnimatedFigure("1.4 GB");
    const missing = parseAnimatedFigure("∅ 缺失 · 非 0");

    expect(missing).toBeNull();
    // Initial mount never counts from 0.
    expect(shouldAnimateFigureTransition(null, ms100)).toBe(false);
    // Same unit (including NBSP vs space) animates when value changes.
    expect(shouldAnimateFigureTransition(ms100, ms84)).toBe(true);
    expect(shouldAnimateFigureTransition(ms100, ms100)).toBe(false);
    // Different units switch directly without interpolating across incompatible units.
    expect(shouldAnimateFigureTransition(ms100, sec1)).toBe(false);
    expect(shouldAnimateFigureTransition(mb512, gb1)).toBe(false);
    expect(shouldAnimateFigureTransition(missing, ms100)).toBe(false);

    const renderedMissing = renderToStaticMarkup(<AnimatedFigure value="∅ 缺失 · 非 0" />);
    expect(renderedMissing).toBe("<span>∅ 缺失 · 非 0</span>");
  });
});
