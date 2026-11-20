// Task 82 (boards/02-overview.html): the alerts summary card on the Overview page.
// Shows only what the status poll already returned; an unknown count stays unknown (never 0).
import type { JSX } from "react";
import { AnimatedFigure, Button, Card, StateStamp } from "../../design";
import { useLocale } from "../../i18n/locale";
import "./overview.css";

export function AlertsSummaryCard({
  open,
  loadPaused,
  onOpen,
}: {
  open: string | null;
  loadPaused: number | null;
  onOpen(): void;
}): JSX.Element {
  const { text } = useLocale();
  const parsed = open === null ? null : Number(open);
  const count = parsed !== null && Number.isFinite(parsed) ? parsed : null;

  const alertDot = count === null
    ? "status-dot status-dot--hollow"
    : count > 0
      ? "status-dot status-dot--critical"
      : "status-dot status-dot--ok";

  return (
    <Card
      actions={
        <Button onClick={onOpen} size="sm" variant="secondary">
          {text("Open alerts →", "前往告警处置 →")}
        </Button>
      }
      className="overview-alerts-card"
      scroll={false}
      title={text("Alerts", "告警")}
      variant="detail"
    >
      <div className="overview-alerts-stack">
        <div className="overview-alerts-item">
          <span className="status-inline">
            <span aria-hidden="true" className={alertDot} />
            <strong>{text("Unacknowledged alerts", "未确认新告警")}</strong>
          </span>
          {count === null ? (
            <StateStamp state="missing" />
          ) : (
            <span className={count > 0 ? "mono overview-alerts-val overview-alerts-hot" : "mono overview-alerts-val"}>
              <AnimatedFigure value={String(count)} />
            </span>
          )}
        </div>

        {/* Known only after the load page read the jobs in this session; otherwise left out. */}
        {loadPaused !== null ? (
          <div className="overview-alerts-item">
            <span className="status-inline">
              <span aria-hidden="true" className={loadPaused > 0 ? "status-dot status-dot--warn" : "status-dot status-dot--ok"} />
              <span>{text("Paused Routine Load", "Routine Load 暂停")}</span>
            </span>
            <span className={loadPaused > 0 ? "mono overview-alerts-val overview-alerts-warn" : "mono overview-alerts-val"}>
              <AnimatedFigure value={String(loadPaused)} />
            </span>
          </div>
        ) : null}
      </div>
    </Card>
  );
}
