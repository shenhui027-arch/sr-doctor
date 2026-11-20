// Design board 07-loads: one load's error log, read live when opened, or the
// copy the bounded collector saved in the local SQLite history store.
// Rows contain business data: shown here only, never written to logs, alerts or exports.
import { useEffect, useId, useRef, useState } from "react";
import type { JSX } from "react";
import type {
  LoadErrorCategory,
  LoadErrorCategoryCount,
  LoadErrorLogResponse,
  StoredLoadErrorLogState,
} from "../../../../contracts/generated/doctor-contracts";
import { Button, Dialog, StateStamp } from "../../design";
import { localizedEnum } from "../../i18n/enum-label";
import type { LocalizedText } from "../../i18n/locale";
import { doctorApi } from "../../lib/api";
import { failureId } from "../history/monitor-read";
import { categoryText, isLoadSessionExpired, loadFailureText } from "./load-text";

/** Saved error-log state in the local history store (`"stored"` aliases `"saved"`). */
export type LoadHistoryErrorLogState = StoredLoadErrorLogState | "stored";

/** One job of one connection: a log is never read for another connection's job.
 * `stored` reads the saved copy from the local history store instead. */
export type ErrorLogTarget = {
  connectionId: string | null;
  jobId: string;
  title: string;
  meta: string;
  stored?: boolean;
  storedState?: LoadHistoryErrorLogState;
  target?: string | null;
};

function SampleCategories({
  categories,
  text,
}: {
  categories: readonly LoadErrorCategoryCount[];
  text: LocalizedText;
}): JSX.Element | null {
  if (categories.length === 0) return null;
  return (
    <section aria-labelledby="load-error-cats" className="load-section">
      <div className="load-cats-head">
        <span className="field-label" id="load-error-cats">
          {text("Sampled reason breakdown:", "样本原因分布：")}
        </span>
        <span className="card-subtitle">
          {text("counted over the sample, not every filtered row", "按样本统计，不代表全部被过滤行")}
        </span>
      </div>
      <div className="load-cats">
        {categories.map((item) => {
          const colDetail = [item.column, item.target_type].filter(Boolean).join(" · ");
          return (
            <span
              className="badge-stamp"
              key={`${item.category}:${item.column ?? ""}:${item.target_type ?? ""}`}
            >
              <strong>{categoryText(item.category, text)}</strong>
              {colDetail ? <span>· {colDetail}</span> : null}
              <span>· {text(`${item.count} rows`, `${item.count} 条`)}</span>
            </span>
          );
        })}
      </div>
    </section>
  );
}

function StoredLogStateNotice({
  state,
  categories,
  text,
}: {
  state: Exclude<LoadHistoryErrorLogState, "saved" | "stored">;
  categories: readonly LoadErrorCategoryCount[];
  text: LocalizedText;
}): JSX.Element {
  switch (state) {
    case "trimmed":
      return (
        <div className="detail-stack">
          <div className="load-purged" data-stored-state="trimmed">
            <strong>
              {text(
                "The saved plaintext was evicted under the 50 MB cap",
                "保存的原文已按 50 MB 上限淘汰",
              )}
            </strong>
            <span>
              {text(
                "Plaintext error logs are capped at 50 MB in total (oldest evicted first). Sampled reason counts remain preserved.",
                "错误原文总量上限 50 MB，超出时先淘汰最旧的原文；样本原因计数仍保留。",
              )}
            </span>
          </div>
          <SampleCategories categories={categories} text={text} />
        </div>
      );
    case "purged":
      return (
        <div className="load-purged" data-stored-state="purged">
          <strong>
            {text(
              "StarRocks already cleared this tracking log before collection",
              "读取前已被 StarRocks 清理",
            )}
          </strong>
          <span>
            {text(
              "StarRocks removed this error tracking log on the BE before the next collection cycle could read it.",
              "下一轮采集读取前，StarRocks 已在 BE 上清理了这份错误日志。",
            )}
          </span>
        </div>
      );
    case "expired":
      return (
        <div className="load-purged" data-stored-state="expired">
          <strong>
            {text(
              "Expired past StarRocks' ~48-hour retention window",
              "已超过 StarRocks 约 48 小时保留期，未能读取",
            )}
          </strong>
          <span>
            {text(
              "BEs keep error logs for about 48 hours by default (load_error_log_reserve_hours); this load aged past that window before SR Doctor read it.",
              "BE 默认保留错误日志约 48 小时（load_error_log_reserve_hours），该导入在读取前已超过保留期。",
            )}
          </span>
        </div>
      );
    case "pending":
      return (
        <div className="load-purged" data-stored-state="pending">
          <strong>
            {text(
              "Waiting for background collection to read this error log",
              "等待后台采集读取这份错误日志",
            )}
          </strong>
          <span>
            {text(
              "Finished loads with filtered rows are queued and read by the bounded collector (up to 5 logs per cycle).",
              "有过滤行的已结束导入已进入待读队列，将由有界采集任务按轮读取（每轮最多 5 份）。",
            )}
          </span>
        </div>
      );
    case "failed":
      return (
        <div className="notice-banner notice-banner--danger" data-stored-state="failed" role="alert">
          <span>
            <strong>{text("Reading this error log failed repeatedly: ", "多次读取错误日志失败：")}</strong>
            {text(
              "The collector could not fetch this tracking log within its retry budget (BE unreachable or timed out).",
              "采集任务在有限重试次数内未能从 BE 读回该错误日志（BE 不可达或读取超时）。",
            )}
          </span>
        </div>
      );
    case "no_log":
      return (
        <div className="state-box" data-stored-state="no_log">
          <strong>{text("No error tracking log for this load", "此次导入未生成错误日志")}</strong>
          <span>
            {text(
              "StarRocks recorded no load_tracking_logs entries for this load.",
              "StarRocks 没有记录此次导入的错误行日志。",
            )}
          </span>
        </div>
      );
    default:
      return (
        <div className="state-box" data-stored-state="unknown">
          <StateStamp detail={localizedEnum(String(state), text)} state="unknown_version" />
        </div>
      );
  }
}

export function ErrorLogBody({
  log,
  storedState,
  text,
}: {
  log: LoadErrorLogResponse;
  storedState?: LoadHistoryErrorLogState;
  text: LocalizedText;
}): JSX.Element {
  const [filter, setFilter] = useState<LoadErrorCategory | "all">("all");
  const filterId = useId();

  if (storedState !== undefined && storedState !== "saved" && storedState !== "stored") {
    return <StoredLogStateNotice categories={log.categories} state={storedState} text={text} />;
  }

  if (log.state === "purged") {
    return (
      <div className="load-purged">
        <strong>
          {text("StarRocks already cleaned up this error log", "错误日志已被 StarRocks 清理")}
        </strong>
        <span>
          {text(
            "BEs keep error logs for 48 hours by default (load_error_log_reserve_hours). Only the error summary recorded on the FE remains.",
            "BE 默认保留错误日志 48 小时（load_error_log_reserve_hours），超过后只剩 FE 记录的错误摘要。",
          )}
        </span>
        <span className="card-subtitle">
          {text(
            "If SR Doctor was collecting then, check 30-day history › Error log copies.",
            "若当时开启了采集，可在“30 天历史 › 错误日志副本”查看已保存副本。",
          )}
        </span>
      </div>
    );
  }

  if (log.state === "unavailable") {
    return (
      <div className="notice-banner notice-banner--danger" role="alert">
        <span>
          {text(
            "StarRocks could not reach the BE for this error log just now (timeout or BE unavailable). The file may still exist; try again.",
            "StarRocks 暂时没能从 BE 读到这份错误日志（超时或 BE 不可用），文件可能仍在，请稍后重试。",
          )}
        </span>
      </div>
    );
  }

  if (log.state === "none" || log.entries.length === 0) {
    return (
      <div className="state-box">
        <strong>
          {storedState === "saved" || storedState === "stored"
            ? text("Saved error log contains no sampled rows.", "已保存的错误日志副本中没有采样行。")
            : text("StarRocks has no error log for this load.", "StarRocks 没有这次导入的错误日志。")}
        </strong>
      </div>
    );
  }

  const entries = log.entries.filter((entry) => filter === "all" || entry.category === filter);
  const categories = [...new Set(log.entries.map((entry) => entry.category))];

  return (
    <div className="detail-stack">
      <SampleCategories categories={log.categories} text={text} />

      <section aria-labelledby="load-error-rows" className="load-section">
        <div className="load-entries-toolbar">
          <h3 className="detail-sec-title" id="load-error-rows">
            {text("Sampled error rows", "逐行错误明细")}{" "}
            <span className="card-subtitle">
              {log.truncated
                ? text(`first ${log.entries.length}, truncated`, `前 ${log.entries.length} 条，已截断`)
                : text(`${log.entries.length} in the log`, `共 ${log.entries.length} 条`)}
            </span>
          </h3>
          {categories.length > 1 ? (
            <div className="filter-chip-input">
              <label htmlFor={filterId}>{text("Reason", "原因")}</label>
              <select
                id={filterId}
                onChange={(event) => setFilter(event.target.value as LoadErrorCategory | "all")}
                value={filter}
              >
                <option value="all">{text("All", "全部")}</option>
                {categories.map((category) => (
                  <option key={category} value={category}>
                    {categoryText(category, text)}
                  </option>
                ))}
              </select>
            </div>
          ) : null}
        </div>

        <ol className="load-entries">
          {entries.map((entry) => {
            const colDetail = [entry.column, entry.target_type].filter(Boolean).join(" · ");
            return (
              <li key={entry.index}>
                <div className="load-entry-head">
                  <span className="status-inline">
                    <span aria-hidden="true" className="status-dot status-dot--critical" />
                    <strong>
                      #{entry.index} {categoryText(entry.category, text)}
                    </strong>
                    {colDetail ? <code className="mono">{colDetail}</code> : null}
                  </span>
                  <span className="load-entry-stamps">
                    {entry.row_converted ? (
                      <span className="state-stamp">{text("Converted values", "转换后的值")}</span>
                    ) : null}
                    {entry.truncated ? (
                      <span className="state-stamp state-stamp--warn">{text("Truncated", "已截断")}</span>
                    ) : (
                      <span className="badge-stamp">{text("Within 4 KB", "单条 4 KB 内")}</span>
                    )}
                  </span>
                </div>
                <div className="load-reason">{entry.reason}</div>
                {entry.row !== null ? <pre className="sql-well load-row">{entry.row}</pre> : null}
              </li>
            );
          })}
        </ol>
      </section>
    </div>
  );
}

export function LoadErrorLogDialog({
  connectionId,
  target,
  onClose,
  onSessionExpired,
  text,
}: {
  connectionId: string | null;
  target: ErrorLogTarget;
  onClose(): void;
  onSessionExpired?(): void;
  text: LocalizedText;
}): JSX.Element {
  const [log, setLog] = useState<LoadErrorLogResponse | null>(null);
  const [storedState, setStoredState] = useState<LoadHistoryErrorLogState | undefined>(target.storedState);
  const [error, setError] = useState<string | null>(null);
  const onSessionExpiredRef = useRef(onSessionExpired);
  useEffect(() => {
    onSessionExpiredRef.current = onSessionExpired;
  });

  useEffect(() => {
    const controller = new AbortController();
    setLog(null);
    setError(null);
    setStoredState(target.storedState);

    const read: Promise<LoadErrorLogResponse> = target.stored
      ? doctorApi
          .readLoadHistoryErrorLog(
            { connection_id: connectionId, target: target.target ?? null, job_id: target.jobId },
            controller.signal,
          )
          .then((saved) => {
            if (!controller.signal.aborted) setStoredState(saved.log.state);
            return {
              job_id: saved.log.job_id,
              state: saved.entries.length ? "available" : "none",
              entries: saved.entries,
              categories: saved.log.categories,
              truncated: saved.log.truncated,
              limitations: saved.limitations,
            };
          })
      : connectionId
        ? doctorApi.readLoadErrorLog(connectionId, target.jobId, controller.signal)
        : Promise.reject({ code: "connection_expired", message_id: "connection_expired" });

    read
      .then((value) => {
        if (!controller.signal.aborted) setLog(value);
      })
      .catch((cause: unknown) => {
        if (controller.signal.aborted) return;
        if (!target.stored && isLoadSessionExpired(cause)) {
          onSessionExpiredRef.current?.();
          setError("connection_expired");
          return;
        }
        setError(failureId(cause, "request_failed"));
      });

    return () => controller.abort();
  }, [connectionId, target.jobId, target.stored, target.storedState, target.target]);

  return (
    <Dialog
      description={<span className="mono">{target.meta}</span>}
      footer={
        <>
          <span>
            {text(
              "StarRocks records only the first error of each row (up to ~50 rows per load)",
              "StarRocks 每行仅记录首个错误（每次导入最多约 50 条）",
            )}
          </span>
          <Button onClick={onClose} variant="primary">
            {text("Close", "关闭")}
          </Button>
        </>
      }
      onOpenChange={(nextOpen) => {
        if (!nextOpen) onClose();
      }}
      open
      title={`${text("Error log", "错误日志")} · ${target.title}`}
      width="lg"
    >
      <div className="detail-stack">
        {error ? (
          <div className="notice-banner notice-banner--danger" role="alert">
            <span>{loadFailureText(error, text)}</span>
          </div>
        ) : log ? (
          <ErrorLogBody log={log} storedState={storedState} text={text} />
        ) : (
          <p className="workspace-loading" role="status">
            {text("Reading the error log…", "正在读取错误日志…")}
          </p>
        )}

        <div className="notice-banner notice-banner--warn">
          <span>
            <strong>
              {text("Read-only access & plaintext storage boundary: ", "数据读取与明文保存边界：")}
            </strong>
            {target.stored
              ? text(
                  "Saved copy read by the bounded collector from information_schema.load_tracking_logs (SR Doctor never connects directly to BE port 8040). Raw rows contain business data and are stored in plaintext in the local SQLite history DB (kept 30 days, 50 MB total cap, up to 50 rows per load, 4 KB per row), deleted when clearing history, and never written to logs, alert notifications or exports.",
                  "本机历史库保存的副本（通过只读会话读取 information_schema.load_tracking_logs，SR Doctor 不直连 BE 8040 端口）。原始行含业务数据，采集期间明文存入本机 SQLite 历史库（保留 30 天、总上限 50\u00A0MB、每次最多 50 条、单条 4\u00A0KB 截断），清空历史时一并删除，绝不进入日志、告警通知与导出文件。",
                )
              : text(
                  "Read via read-only session from information_schema.load_tracking_logs (SR Doctor never connects directly to BE port 8040). Raw rows contain business data; during collection they are stored in plaintext in the local SQLite history DB (kept 30 days, 50 MB total cap, up to 50 rows per load, 4 KB per row), deleted when clearing history, and never written to logs, alert notifications or exports.",
                  "日志通过只读会话读取 information_schema.load_tracking_logs（SR Doctor 不直连 BE 8040 端口）。原始行含业务数据，采集期间明文存入本机 SQLite 历史库（保留 30 天、总上限 50\u00A0MB、每次最多 50 条、单条 4\u00A0KB 截断），清空历史时一并删除，绝不进入日志、告警通知与导出文件。",
                )}
          </span>
        </div>
      </div>
    </Dialog>
  );
}
