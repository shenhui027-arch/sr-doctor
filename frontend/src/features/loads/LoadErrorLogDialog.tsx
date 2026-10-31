// Design board I3: one load's error log, read live when opened, or (task 58)
// the copy the collector saved. Rows are business data: shown here only,
// never cached, logged or exported.
import { useEffect, useRef, useState } from "react";
import type { JSX, KeyboardEvent } from "react";
import type { LoadErrorCategory, LoadErrorLogResponse, StoredLoadErrorLogState } from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { doctorApi } from "../../lib/api";
import { failureId } from "../history/monitor-read";
import { categoryText, loadFailureText } from "./load-text";

/** One job of one connection: a log is never read for another connection's job.
 * `stored` reads the saved copy from the local history store instead. */
export type ErrorLogTarget = { connectionId: string | null; jobId: string; title: string; meta: string; stored?: boolean; target?: string | null };

export function ErrorLogBody({ log, storedState, text }: { log: LoadErrorLogResponse; storedState?: StoredLoadErrorLogState; text: LocalizedText }): JSX.Element {
  const [filter, setFilter] = useState<LoadErrorCategory | "all">("all");
  if (storedState === "trimmed") {
    return <div className="load-purged">
      <strong>{text("The saved text was dropped under the 50 MB limit", "保存的原文已按 50 MB 上限删除")}</strong>
      <span>{text("Error log text is kept up to 50 MB in all; the oldest goes first. The reason counts are kept.", "错误原文总量上限 50 MB，超出时先删最旧的；原因计数仍保留。")}</span>
    </div>;
  }
  if (log.state === "purged") {
    return <div className="load-purged">
      <strong>{text("StarRocks already cleaned up this error log", "错误日志已被 StarRocks 清理")}</strong>
      <span>{text("BEs keep error logs for 48 hours by default (load_error_log_reserve_hours). Only the error message StarRocks recorded is left.", "BE 默认保留错误日志 48 小时（load_error_log_reserve_hours），超过后只剩 StarRocks 记录的错误信息。")}</span>
      <span className="history-muted">{text("If SR Doctor was collecting then, a saved copy may be under History › Error logs.", "如果当时 SR Doctor 正在采集，可在“历史 › 错误日志”里查看保存的副本。")}</span>
    </div>;
  }
  if (log.state === "unavailable") {
    return <p className="capability-error" role="alert">{text("StarRocks could not reach the BE for this error log just now (timeout or BE unavailable). The file may still exist; try again.", "StarRocks 暂时没能从 BE 读到这份错误日志（超时或 BE 不可用），文件可能仍在，请稍后重试。")}</p>;
  }
  if (log.state === "none" || log.entries.length === 0) {
    return <p className="notify-empty">{text("StarRocks has no error log for this load.", "StarRocks 没有这次导入的错误日志。")}</p>;
  }
  const entries = log.entries.filter((entry) => filter === "all" || entry.category === filter);
  const categories = [...new Set(log.entries.map((entry) => entry.category))];
  return <>
    <section aria-labelledby="load-error-cats" className="load-section">
      <h3 id="load-error-cats">{text("Reasons", "原因分布")} <span className="history-muted">{text("counted over the sample, not every filtered row", "按样本统计，不代表全部被过滤行")}</span></h3>
      <div className="load-cats">
        {log.categories.map((item) => <span className="load-cat" key={`${item.category}:${item.column ?? ""}:${item.target_type ?? ""}`}>
          <strong>{categoryText(item.category, text)}</strong>
          {(item.column || item.target_type) && <span>{[item.column, item.target_type].filter(Boolean).join(" · ")}</span>}
          <span className="mono">{item.count}</span>
        </span>)}
      </div>
    </section>
    <section aria-labelledby="load-error-rows" className="load-section">
      <div className="history-card-head">
        <h3 id="load-error-rows">{text("Rows", "逐行错误")} <span className="history-muted">{log.truncated ? text(`first ${log.entries.length}, truncated`, `前 ${log.entries.length} 条，已截断`) : text(`${log.entries.length} in the log`, `日志中共 ${log.entries.length} 条`)}</span></h3>
        <label className="notify-filter">{text("Reason", "原因")}
          <select onChange={(event) => setFilter(event.target.value as LoadErrorCategory | "all")} value={filter}>
            <option value="all">{text("All", "全部")}</option>
            {categories.map((category) => <option key={category} value={category}>{categoryText(category, text)}</option>)}
          </select>
        </label>
      </div>
      <ol className="load-entries">
        {entries.map((entry) => <li key={entry.index}>
          <div className="load-entry-head">
            <span className="mono history-muted">#{entry.index}</span>
            <span className="notify-chip bad">{categoryText(entry.category, text)}</span>
            {(entry.column || entry.target_type) && <span className="mono">{[entry.column, entry.target_type].filter(Boolean).join(" · ")}</span>}
            {entry.row_converted && <span className="planned">{text("converted values", "转换后的值")}</span>}
            {entry.truncated && <span className="planned">{text("truncated", "已截断")}</span>}
          </div>
          <div className="load-reason">{entry.reason}</div>
          {entry.row !== null && <pre className="load-row">{entry.row}</pre>}
        </li>)}
      </ol>
    </section>
  </>;
}

export function LoadErrorLogDialog({ connectionId, target, onClose, text }: {
  connectionId: string | null;
  target: ErrorLogTarget;
  onClose(): void;
  text: LocalizedText;
}): JSX.Element {
  const [log, setLog] = useState<LoadErrorLogResponse | null>(null);
  const [storedState, setStoredState] = useState<StoredLoadErrorLogState | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);
  const dialog = useRef<HTMLDivElement>(null);
  // Focus moves into the dialog, stays there while it is open (aria-modal)
  // and returns to the button that opened it.
  useEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    dialog.current?.focus();
    return () => { opener?.focus(); };
  }, []);
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") { event.preventDefault(); onClose(); return; }
    if (event.key !== "Tab" || !dialog.current) return;
    const items = Array.from(dialog.current.querySelectorAll<HTMLElement>("button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])"));
    if (items.length === 0) return;
    const first = items[0];
    const last = items[items.length - 1];
    const inside = dialog.current.contains(document.activeElement);
    if (event.shiftKey && (document.activeElement === first || !inside || document.activeElement === dialog.current)) { event.preventDefault(); last.focus(); }
    else if (!event.shiftKey && (document.activeElement === last || !inside)) { event.preventDefault(); first.focus(); }
  };
  useEffect(() => {
    const controller = new AbortController();
    const read: Promise<LoadErrorLogResponse> = target.stored
      ? doctorApi.readLoadHistoryErrorLog({ connection_id: connectionId, target: target.target ?? null, job_id: target.jobId }, controller.signal).then((saved) => {
        if (!controller.signal.aborted) setStoredState(saved.log.state);
        return { job_id: saved.log.job_id, state: saved.entries.length ? "available" : "none", entries: saved.entries, categories: saved.log.categories, truncated: saved.log.truncated, limitations: saved.limitations };
      })
      : connectionId ? doctorApi.readLoadErrorLog(connectionId, target.jobId, controller.signal) : Promise.reject({ code: "connection_expired" });
    read
      .then((value) => { if (!controller.signal.aborted) setLog(value); })
      .catch((cause: unknown) => { if (!controller.signal.aborted) setError(failureId(cause, "request_failed")); });
    return () => controller.abort();
  }, [connectionId, target.jobId, target.stored]);
  return <div className="notify-overlay" onKeyDown={onKeyDown} role="presentation">
    <div aria-labelledby="load-error-title" aria-modal="true" className="notify-dialog load-error-dialog" ref={dialog} role="dialog" tabIndex={-1}>
      <div className="notify-dialog-head">
        <div>
          <h2 id="load-error-title">{text("Error log", "错误日志")} · {target.title}</h2>
          <span className="history-muted">{target.meta}</span>
        </div>
        <button aria-label={text("Close", "关闭")} className="notify-close" onClick={onClose} type="button">
          <svg aria-hidden="true" fill="none" height="18" stroke="currentColor" strokeLinecap="round" strokeWidth="2" viewBox="0 0 24 24" width="18"><path d="M6 6l12 12M18 6L6 18" /></svg>
        </button>
      </div>
      <div className="notify-dialog-body">
        {error ? <p className="capability-error" role="alert">{loadFailureText(error, text)}</p>
          : log ? <ErrorLogBody log={log} storedState={storedState} text={text} />
            : <p className="workspace-loading" role="status">{text("Reading the error log…", "正在读取错误日志…")}</p>}
        <ul className="notify-notes boxed">
          <li>{target.stored
            ? text("A copy the collector read from information_schema.load_tracking_logs after the load finished and saved in the local history store.", "采集任务在导入结束后从 information_schema.load_tracking_logs 读取、保存在本机历史库里的副本。")
            : text("Read from StarRocks information_schema.load_tracking_logs for this job; it is the same text as the ErrorURL. SR Doctor does not reach the BE HTTP port.", "内容由 StarRocks 的 information_schema.load_tracking_logs 按这次导入的 job_id 读取，与 ErrorURL 的内容一致；SR Doctor 不直接访问 BE 的 8040 端口。")}</li>
          <li>{text("StarRocks keeps about 50 error rows per load and only the first error of each row.", "StarRocks 每次导入最多记录约 50 行错误，每行只报第一个错误。")}</li>
          <li>{text("Rows are business data. While collecting, loads with filtered rows have a copy saved in plain text in the local history store (30 days, 50 MB in all); it never goes to logs, notifications or exports.", "原始行含业务数据。采集期间，有过滤行的导入会按约定把一份副本明文保存在本机历史库（保留 30 天、总量 50 MB），不进日志、告警通知和导出。")}</li>
        </ul>
      </div>
      <div className="notify-dialog-foot">
        <span className="notify-spacer" />
        <button className="button accent" onClick={onClose} type="button">{text("Close", "关闭")}</button>
      </div>
    </div>
  </div>;
}
