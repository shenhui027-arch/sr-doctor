// Design board 07-loads / 02-overview: compact load summary card inside the audit overview.
// Shows what StarRocks holds now (~3 days / ~1000 records), independent of the audit window.
import { useEffect, useState } from "react";
import type { JSX } from "react";
import type { LoadOverviewResponse } from "../../../../contracts/generated/doctor-contracts";
import { Button } from "../../design";
import type { LocalizedText } from "../../i18n/locale";
import type { DoctorApi } from "../../lib/api";
import type { LoadFocus } from "./LoadWorkspace";
import { count, feTime, hourText, ms, shareText } from "./load-text";
import "./loads.css";

type CardState = {
  key: string;
  overview: LoadOverviewResponse | null;
  error: string | null;
  readAt: number | null;
};

export function LoadSummaryCard({
  api,
  connectionId,
  describeFailure,
  onOpen,
  text,
}: {
  api: Pick<DoctorApi, "readLoadOverview">;
  connectionId: string;
  describeFailure(cause: unknown): string;
  onOpen(focus: LoadFocus): void;
  text: LocalizedText;
}): JSX.Element {
  const [attempt, setAttempt] = useState(0);
  const [state, setState] = useState<CardState | null>(null);
  const key = `${connectionId}:${attempt}`;
  useEffect(() => {
    const controller = new AbortController();
    api
      .readLoadOverview({ connection_id: connectionId, kind: null, include_system: false }, controller.signal)
      .then((overview) => {
        if (!controller.signal.aborted) setState({ key, overview, error: null, readAt: Date.now() });
      })
      .catch((cause: unknown) => {
        if (!controller.signal.aborted) setState({ key, overview: null, error: describeFailure(cause), readAt: null });
      });
    return () => controller.abort();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  const current = state?.key === key ? state : null;
  return (
    <LoadSummaryView
      current={current && { overview: current.overview, error: current.error, readAt: current.readAt }}
      onOpen={onOpen}
      onRetry={() => setAttempt((value) => value + 1)}
      text={text}
    />
  );
}

export function LoadSummaryView({
  current,
  onOpen,
  onRetry,
  text,
}: {
  current: { overview: LoadOverviewResponse | null; error: string | null; readAt?: number | null } | null;
  onOpen(focus: LoadFocus): void;
  onRetry(): void;
  text: LocalizedText;
}): JSX.Element {
  const head = (subtitle: string | null) => (
    <header className="card-hd">
      <div className="card-title-group">
        <h2 className="card-title" id="audit-load-card">
          {text("Loads", "导入")}
        </h2>
        {subtitle ? <span className="load-muted">{subtitle}</span> : null}
      </div>
      {current?.overview ? (
        <div className="card-hd-actions load-summary-meta">
          {current.readAt ? (
            <span className="load-muted">
              {text(
                `read at ${new Date(current.readAt).toLocaleTimeString("en-GB")} (this computer's clock)`,
                `读取于 ${new Date(current.readAt).toLocaleTimeString("zh-CN", { hour12: false })}（本机时间）`,
              )}
            </span>
          ) : null}
          <Button onClick={onRetry} size="sm" variant="secondary">
            {text("Refresh", "刷新")}
          </Button>
          <Button onClick={() => onOpen({ view: "overview" })} size="sm" variant="primary">
            {text("Open the load overview", "打开导入概览")}
          </Button>
        </div>
      ) : null}
    </header>
  );

  if (!current) {
    return (
      <section aria-busy="true" aria-labelledby="audit-load-card" className="card load-summary-card">
        {head(null)}
        <div className="card-bd">
          <p className="load-muted" role="status">
            {text("Reading load records…", "正在读取导入信息…")}
          </p>
        </div>
      </section>
    );
  }
  if (current.error || !current.overview) {
    return (
      <section aria-labelledby="audit-load-card" className="card load-summary-card">
        {head(null)}
        <div className="card-bd load-inline-filters">
          <p className="load-num-critical" role="alert">
            {text(
              `Load information could not be read (${current.error}). The audit overview above is not affected.`,
              `读取导入信息失败（${current.error}），不影响上方审计概览。`,
            )}
          </p>
          <Button onClick={onRetry} size="sm" variant="secondary">
            {text("Retry", "重试")}
          </Button>
        </div>
      </section>
    );
  }
  const overview = current.overview;
  const routine = overview.routine;
  const records = overview.support !== "unsupported_version";
  const routineShown = routine.support !== "unsupported_version";
  if (!records && !routineShown) {
    return (
      <section aria-labelledby="audit-load-card" className="card load-summary-card">
        {head(null)}
        <div className="card-bd">
          <p className="load-muted">
            {text(
              "Load records need StarRocks 3.3 or later and Routine Load 3.2 or later.",
              "导入记录需要 StarRocks 3.3 及以上，Routine Load 需要 3.2 及以上。",
            )}
          </p>
        </div>
      </section>
    );
  }
  const noJobs = routine.running === "0" && routine.scheduling === "0" && routine.paused === "0" && routine.ended === "0";
  if (records && overview.summary.loads === "0" && noJobs) {
    return (
      <section aria-labelledby="audit-load-card" className="card load-summary-card">
        {head(null)}
        <div className="card-bd">
          <p className="load-muted">
            {text(
              "StarRocks holds no load record and no Routine Load job right now.",
              "StarRocks 当前没有保留任何导入记录，也没有 Routine Load 作业。",
            )}
          </p>
        </div>
      </section>
    );
  }
  const summary = overview.summary;
  const pausedJob = routine.paused_jobs[0] ?? null;
  const hours = overview.hours;

  return (
    <section aria-labelledby="audit-load-card" className="card load-summary-card">
      {head(
        text(
          "what StarRocks holds now · last 3 days / about 1000 records · does not follow the window above",
          "StarRocks 当前保留的记录 · 最近 3 天 / 约 1000 条 · 不随上方时间窗变化",
        ),
      )}
      <div className="card-bd load-summary-body">
        {records ? (
          <dl className="load-summary-facts">
            <div>
              <dt>{text("Loads", "导入次数")}</dt>
              <dd>{count(summary.loads)}</dd>
            </div>
            <div>
              <dt>{text("Failed / cancelled", "失败 / 取消")}</dt>
              <dd className={summary.failed !== "0" ? "load-num-critical" : undefined}>
                {count(summary.failed)}
                {shareText(overview.failed_share_permyriad) ? <small> {shareText(overview.failed_share_permyriad)}</small> : null}
              </dd>
            </div>
            <div>
              <dt>{text("Filtered rows", "过滤行")}</dt>
              <dd>
                {count(summary.filtered_rows)}
                {summary.filtered_rows_unknown !== "0" ? (
                  <small> {text(`+${summary.filtered_rows_unknown} unknown`, `另 ${summary.filtered_rows_unknown} 条未知`)}</small>
                ) : null}
              </dd>
            </div>
            <div>
              <dt>{text("Begin txn + plan P95", "取事务 + 规划 P95")}</dt>
              <dd>
                {summary.begin_plan_p95_ms !== null ? (
                  ms(summary.begin_plan_p95_ms)
                ) : (
                  <>
                    —<small> {text("fewer than 20 samples", "样本不足 20 条")}</small>
                  </>
                )}
              </dd>
            </div>
          </dl>
        ) : (
          <p className="load-muted">
            {text(
              "Load records need StarRocks 3.3 or later; only Routine Load is shown.",
              "导入记录需要 3.3 及以上；只显示 Routine Load 的状态。",
            )}
          </p>
        )}
        {records && hours.length > 0 ? (
          <div className="load-summary-spark">
            <span className="load-muted">
              {text(`Loads per hour (${overview.fe_time_zone ?? "FE time zone"})`, `每小时导入次数（${overview.fe_time_zone ?? "FE 时区"}）`)}
            </span>
            <div
              aria-label={text(
                `Hourly loads from ${hourText(hours[0].hour)} to ${hourText(hours[hours.length - 1].hour)}`,
                `${hourText(hours[0].hour)} 到 ${hourText(hours[hours.length - 1].hour)} 每小时导入次数`,
              )}
              className="load-spark"
              role="img"
            >
              {hours.map((hour) => (
                <span className={hour.coverage === "complete" ? undefined : hour.coverage === "unknown" ? "gone" : "partial"} key={hour.hour}>
                  <span className="fail" style={{ height: `${hour.failed_permyriad / 100}%` }} />
                  <span className="okay" style={{ height: `${(hour.loads_permyriad - hour.failed_permyriad) / 100}%` }} />
                </span>
              ))}
            </div>
            <div aria-hidden="true" className="load-hour-labels">
              <span>{hourText(hours[0].hour)}</span>
              <span>{hourText(hours[hours.length - 1].hour)}</span>
            </div>
            <div className="load-summary-legend">
              <span><i aria-hidden="true" className="ok" />{text("loads", "导入")}</span>
              <span><i aria-hidden="true" className="bad" />{text("failed", "失败")}</span>
              {hours.some((hour) => hour.coverage === "unknown") ? (
                <span>
                  <i aria-hidden="true" className="gone" />
                  {text(`unknown: before ${feTime(overview.earliest_create_time)}`, `未知：早于 ${feTime(overview.earliest_create_time)}`)}
                </span>
              ) : null}
              {hours.some((hour) => hour.coverage === "partial") ? (
                <span>{text("striped: possibly incomplete", "斜纹：可能不完整")}</span>
              ) : null}
            </div>
          </div>
        ) : null}
        {routineShown ? (
          <div className="load-summary-routine">
            <span className="load-muted">
              Routine Load · {text(`${routine.running} running`, `${routine.running} 运行`)} ·{" "}
              <span className={routine.paused !== "0" ? "load-num-critical" : undefined}>
                {text(`${routine.paused} paused`, `${routine.paused} 暂停`)}
              </span>
            </span>
            {pausedJob ? (
              <button
                className="load-reason-box load-summary-paused"
                onClick={() => onOpen({ view: "routine", jobId: pausedJob.job_id })}
                type="button"
              >
                <strong>
                  {text(`${pausedJob.name} paused`, `${pausedJob.name} 已暂停`)}
                  {pausedJob.max_lag ? text(` · lag ${count(pausedJob.max_lag)}`, ` · 积压 ${count(pausedJob.max_lag)}`) : ""}
                </strong>
                <span className="mono">
                  {pausedJob.reason ?? text("no reason reported", "StarRocks 没有给出原因")}
                  {pausedJob.pause_time ? ` · ${feTime(pausedJob.pause_time)}` : ""}
                </span>
              </button>
            ) : null}
            {routine.max_running_lag ? (
              <span className="load-muted">
                {text("Largest lag (running)", "最大积压（运行中）")}{" "}
                <span className="mono">
                  {routine.max_running_lag.job_name} · p{routine.max_running_lag.partition} · {count(routine.max_running_lag.lag)}
                </span>
              </span>
            ) : null}
            {routine.support === "without_lag" ? (
              <span className="load-muted">{text("StarRocks 3.2 has no lag.", "StarRocks 3.2 没有积压数据。")}</span>
            ) : null}
          </div>
        ) : null}
      </div>
    </section>
  );
}
