import type { Dispatch, JSX } from "react";
import type { AnalysisSnapshotReport } from "../../../../contracts/generated/doctor-contracts";
import { Card, InfoTip, PageGrid } from "../../design";
import { useLocale } from "../../i18n/locale";
import { ConnectionForm } from "../connection/ConnectionForm";
import type { ConnectionAction, ConnectionState } from "../connection/connection-state";
import { ImportPanel } from "../import/ImportPanel";
import "./first-run.css";

/** Not connected yet (boards/01-connect.html): two equal card--form cards side by side. */
export function FirstRunPage({
  connection,
  dispatch,
  maxInputBytes = null,
  onReport,
  onSessionEnded,
  onSessionExpired,
}: {
  connection: ConnectionState;
  dispatch: Dispatch<ConnectionAction>;
  maxInputBytes?: number | null;
  onReport(report: AnalysisSnapshotReport): void;
  onSessionEnded?(): void;
  onSessionExpired?(): void;
}): JSX.Element {
  const { text } = useLocale();

  return (
    <PageGrid
      aria-label={text("Connect StarRocks or import a Profile", "连接 StarRocks 或离线导入 Profile")}
      className="first-run"
    >
      <ConnectionForm
        dispatch={dispatch}
        idPrefix="start"
        onSessionEnded={onSessionEnded}
        onSessionExpired={onSessionExpired}
        renderLayout={({ body, footer }) => (
          <Card
            className="first-run-connect"
            footer={
              <>
                <span className="mono">{text("Read-only session", "只读会话")}</span>
                <div className="connection-footer-actions">{footer}</div>
              </>
            }
            span={{ lg: 6, md: 8, sm: 4 }}
            title={text("Connect StarRocks", "连接 StarRocks")}
            titleExtra={
              <InfoTip label={text("Read-only connection and session boundary", "只读连接与会话边界说明")}>
                {text(
                  "Read-only MySQL connection (multi-statements disabled, no DDL/DML). Credentials live in memory by default and are dropped on close or idle expiry.",
                  "MySQL 协议只读连接（禁用多语句，不发 DDL/DML），口令默认内存驻留、关闭或空闲过期即销毁。",
                )}
              </InfoTip>
            }
            variant="form"
          >
            {body}
          </Card>
        )}
        showCapabilities
        state={connection}
        submitLabel={text("Connect and probe", "连接并探测能力")}
      />

      <ImportPanel
        idPrefix="start-profile"
        maxInputBytes={maxInputBytes}
        onReport={onReport}
        span={{ lg: 6, md: 8, sm: 4 }}
      />
    </PageGrid>
  );
}
