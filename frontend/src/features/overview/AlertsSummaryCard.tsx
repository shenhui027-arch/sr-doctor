// Task 66 (board 2): the alerts card on the Overview page. It shows only what the
// status poll already returned; an unknown count stays unknown (never 0).
import type { JSX } from "react";
import { useLocale } from "../../i18n/locale";
import { AnimatedFigure } from "../shell/motion-enhance";
import "./overview.css";

export function AlertsSummaryCard({ open, loadPaused, onOpen }: { open: string | null; loadPaused: number | null; onOpen(): void }): JSX.Element {
  const { text } = useLocale();
  const count = open === null ? null : Number(open);
  return <section aria-labelledby="overview-alerts-title" className="overview-alerts">
    <div className="overview-alerts-head">
      <h2 id="overview-alerts-title">{text("Alerts", "告警")}</h2>
      <button className="button text" onClick={onOpen} type="button">{text("Open alerts", "去告警")}</button>
    </div>
    <dl className="overview-alerts-facts">
      <div><dt>{text("New", "未处理")}</dt><dd className={count !== null && count > 0 ? "overview-alerts-hot" : undefined}>{count === null ? text("Unknown", "未知") : <AnimatedFigure value={String(count)} />}</dd></div>
      {/* Known only after the load page read the jobs in this session; otherwise the row is left out. */}
      {loadPaused !== null ? <div><dt>{text("Paused Routine Load", "Routine Load 暂停")}</dt><dd className={loadPaused > 0 ? "overview-alerts-hot" : undefined}>{loadPaused}</dd></div> : null}
    </dl>
  </section>;
}
