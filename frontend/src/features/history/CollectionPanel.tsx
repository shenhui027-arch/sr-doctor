// Design board 08-settings (历史库与采集): three cards side by side —
// 1. Incremental collection & read budget (col-4)
// 2. 24h coverage & recent reads (col-5)
// 3. Local SQLite history store with fixed-footer Clear history… + danger Dialog (col-3)
import { useState } from "react";
import type { JSX, ReactNode } from "react";
import type { CollectionRun, CoverageHourState, HistoryStatusResponse } from "../../../../contracts/generated/doctor-contracts";
import { AnimatedFigure, Button, Card, Dialog, EvidenceMeter, InfoTip, PageGrid, type EvidenceLevel } from "../../design";
import { lookupEnumLabel } from "../../i18n/enum-label";
import type { LocalizedText } from "../../i18n/locale";
import { bytesText, countText } from "../../lib/duration-display";
import { clockText, collectionLabel } from "./history-status";
import "./history.css";

export { clockText, collectionLabel };

const COVERAGE_STATE_LABELS: Record<CoverageHourState, readonly [string, string]> = {
  complete: ["Complete", "完整"],
  partial: ["Partial (row limit reached)", "部分（达到行数上限）"],
  not_collected: ["Not collected (not zero executions)", "未采集（不是 0 次执行）"],
};

export function formatHistoryBytes(value: string | null | undefined): string {
  if (!value) return "—";
  return bytesText(value) ?? "—";
}

function runBadge(run: CollectionRun, text: LocalizedText): JSX.Element {
  const reason = run.reason ?? "";
  switch (run.state) {
    case "complete":
      return (
        <span className="status-inline">
          <span aria-hidden="true" className="status-dot status-dot--ok" />
          {text("Complete", "成功")}
        </span>
      );
    case "partial":
      return (
        <span className="state-stamp state-stamp--warn">
          ◐ {reason === "row_limit" ? text("Partial · row limit reached", "部分 · 达到行数上限") : text("Partial", "部分")}
        </span>
      );
    case "gap":
      return <span className="state-stamp">{text("Not read · beyond catch-up", "未读取 · 超出补采范围")}</span>;
    case "failed":
    default:
      return (
        <span className="state-stamp state-stamp--critical">
          {text(`Failed · will retry (${reason})`, `失败 · 稍后重试（${reason}）`)}
        </span>
      );
  }
}

export function CollectionPanel({
  status,
  busy,
  onToggle,
  onClear,
  clearConfirm,
  initialConfirmOpen = false,
  text,
}: {
  status: HistoryStatusResponse;
  busy: boolean;
  onToggle(action: "pause" | "resume"): void;
  onClear(): void;
  /** Optional custom confirmation override; defaults to a danger Dialog. */
  clearConfirm?: ReactNode;
  initialConfirmOpen?: boolean;
  text: LocalizedText;
}): JSX.Element {
  const [confirmOpen, setConfirmOpen] = useState(initialConfirmOpen);
  const state = status.collection;
  const store = status.store;
  const canToggle = state.state === "running" || state.state === "paused" || state.state === "backoff";
  const counts = status.coverage.reduce(
    (total, hour) => ({ ...total, [hour.state]: (total[hour.state] ?? 0) + 1 }),
    {} as Record<string, number>,
  );

  const completeHours = counts.complete ?? 0;
  const totalHours = status.coverage.length;
  const coverageEvidenceLevel: EvidenceLevel = totalHours === 0
    ? "missing"
    : completeHours === totalHours
      ? "complete"
      : completeHours > 0 || (counts.partial ?? 0) > 0
        ? "partial"
        : "sparse";

  const collectorDot = state.state === "running"
    ? "status-dot status-dot--ok"
    : state.state === "backoff"
      ? "status-dot status-dot--warn"
      : "status-dot status-dot--hollow";

  return (
    <>
      <PageGrid className="collection-grid">
        {/* Card 1 (col-4): Incremental collection & read budget */}
        <Card
          actions={
            canToggle ? (
              <Button
                disabled={busy}
                onClick={() => onToggle(state.state === "paused" ? "resume" : "pause")}
                size="sm"
                variant="secondary"
              >
                {state.state === "paused" ? text("Resume", "恢复采集") : text("Pause", "暂停采集")}
              </Button>
            ) : null
          }
          footer={
            <>
              <span>{text("Single bounded connection · pause keeps watermark", "单连接有界读取 · 暂停保留已采水位")}</span>
              <span>{status.remote_collection_enabled ? text("Remote collection: enabled", "远端采集：已启用") : text("Remote collection: off", "远端采集：未启用")}</span>
            </>
          }
          span={{ lg: 4, md: 8, sm: 4 }}
          title={text("Incremental collection", "增量采集")}
          titleExtra={
            <span className="status-inline">
              <span aria-hidden="true" className={collectorDot} />
              <span>{collectionLabel(status, text)}</span>
            </span>
          }
          variant="detail"
        >
          <div className="collection-card-stack">
            {state.state === "stopped" && state.reason === "production_gate" ? (
              <div className="notice-banner notice-banner--warn" role="note">
                <div>
                  <strong>{text("Remote cluster: collection is not enabled yet.", "这是远端集群，采集尚未启用。")}</strong>{" "}
                  {text(
                    "A production read-only pilot must first confirm TLS verify-full, the account's read-only permissions and the read load; the operator then turns collection on.",
                    "需要先完成生产只读试点，确认 TLS verify-full、账号只读权限与读取负载，再由运维开启采集。",
                  )}
                </div>
              </div>
            ) : state.state === "stopped" && state.reason === "not_connected" ? (
              <div className="notice-banner" role="note">
                <span>
                  {text(
                    "No connected session. Reconnect to continue from the watermark; the time in between is shown as not collected.",
                    "当前没有已连接的会话。重新连接后从水位继续；期间的时段显示为未采集。",
                  )}
                </span>
              </div>
            ) : null}

            <dl className="kv-grid">
              <dt>{text("Target", "采集目标")}</dt>
              <dd className="mono">{status.target ?? "—"}</dd>
              <dt>{text("Watermark (UTC)", "采集水位 (UTC)")}</dt>
              <dd className="mono">{clockText(status.watermark_seconds)}</dd>
              <dt>{text("Next read (UTC)", "下次读取 (UTC)")}</dt>
              <dd className="mono">{state.state === "running" || state.state === "backoff" ? clockText(state.next_run_seconds) : "—"}</dd>
              <dt>{text("Catch-up limit", "补采窗口上限")}</dt>
              <dd>
                <span className="mono u-nowrap">{countText(String(status.budget.catch_up_seconds)) ?? status.budget.catch_up_seconds}&nbsp;s</span>
              </dd>
            </dl>

            <div className="collection-subblock">
              <div className="collection-subblock-hd">
                <span className="field-label">{text("Read budget per slice", "每次读取预算")}</span>
                {!status.budget.production_budget_measured ? (
                  <span className="state-stamp state-stamp--warn" title={text("The production budget is set after the production read-only pilot measures it.", "生产预算由生产只读试点实测后写入。")}>
                    {text("Not measured yet", "待实测")}
                  </span>
                ) : null}
              </div>
              <table aria-label={text("Read budget per slice", "每次读取预算表")} className="data-table">
                <thead>
                  <tr>
                    <th scope="col">{text("Item", "预算项")}</th>
                    <th className="num" scope="col">{text("Local default", "本机默认")}</th>
                    <th className="num" scope="col">{text("Production", "生产预算")}</th>
                  </tr>
                </thead>
                <tbody>
                  <tr>
                    <td>{text("Read period", "读取周期")}</td>
                    <td className="num"><span className="u-nowrap">{status.budget.period_seconds}&nbsp;s</span></td>
                    <td className="num muted">—</td>
                  </tr>
                  <tr>
                    <td>{text("Slice length", "单次时间片")}</td>
                    <td className="num"><span className="u-nowrap">{status.budget.slice_seconds}&nbsp;s</span></td>
                    <td className="num muted">—</td>
                  </tr>
                  <tr>
                    <td>{text("Read lag", "读取滞后")}</td>
                    <td className="num"><span className="u-nowrap">{status.budget.lag_seconds}&nbsp;s</span></td>
                    <td className="num muted">—</td>
                  </tr>
                  <tr>
                    <td>{text("Rows per slice", "行数上限")}</td>
                    <td className="num">{countText(String(status.budget.slice_row_limit)) ?? status.budget.slice_row_limit}</td>
                    <td className="num muted">—</td>
                  </tr>
                  <tr>
                    <td>{text("Statement timeout", "语句超时")}</td>
                    <td className="num"><span className="u-nowrap">{status.budget.query_timeout_seconds}&nbsp;s</span></td>
                    <td className="num muted">—</td>
                  </tr>
                </tbody>
              </table>
            </div>

            <details className="disclosure">
              <summary>{text("How collection runs", "采集与有界退避策略")}</summary>
              <dl className="kv-grid collection-disclosure-kv">
                <dt>{text("Bounded read", "单连接有界")}</dt>
                <dd>{text("One connection · deduplicated by queryId · marks slice partial instead of raising load.", "单条连接按 queryId 去重；超预算标为部分采集绝不加压。")}</dd>
                <dt>{text("On failure", "失败退避")}</dt>
                <dd>{text("Exponential backoff without retry storms.", "失败按指数退避重试，不形成重试风暴。")}</dd>
              </dl>
            </details>
          </div>
        </Card>

        {/* Card 2 (col-5): 24h coverage & recent reads */}
        <Card
          actions={
            <span className="card-subtitle">
              {text(
                `complete ${counts.complete ?? 0} · partial ${counts.partial ?? 0} · not collected ${counts.not_collected ?? 0} · gaps are never interpolated`,
                `完整 ${counts.complete ?? 0} · 部分 ${counts.partial ?? 0} · 未采集 ${counts.not_collected ?? 0}（缺口不插值）`,
              )}
            </span>
          }
          className="collection-coverage-card"
          footer={
            <>
              <EvidenceMeter
                label={text(`24h coverage (${completeHours}/${totalHours} complete)`, `24h 证据（${completeHours}/${totalHours} 小时完整）`)}
                level={coverageEvidenceLevel}
              />
              <span>{text("Gaps stay open · never filled as 0", "缺口保持断开 · 绝不按 0 补齐")}</span>
            </>
          }
          span={{ lg: 5, md: 8, sm: 4 }}
          title={text("Coverage · last 24 hours", "24 小时覆盖与最近读取")}
          variant="default"
        >
          <div className="collection-card-stack">
            <div className="collection-strip-block">
              <ol aria-label={text("Hourly coverage", "最近 24 小时每小时覆盖状态")} className="coverage-strip">
                {status.coverage.map((hour) => {
                  const cls = hour.state === "partial"
                    ? "cov-cell cov-cell--partial"
                    : hour.state === "not_collected"
                      ? "cov-cell cov-cell--gap"
                      : "cov-cell";
                  return (
                    <li
                      className={cls}
                      data-coverage-state={hour.state}
                      key={hour.hour_start_seconds}
                      title={`${clockText(hour.hour_start_seconds)} · ${lookupEnumLabel(COVERAGE_STATE_LABELS, hour.state, text)}`}
                    />
                  );
                })}
              </ol>
              <div className="collection-legend">
                <span><i aria-hidden="true" className="cov-swatch cov-cell" />{text("Complete", "完整")}</span>
                <span><i aria-hidden="true" className="cov-swatch cov-cell cov-cell--partial" />{text("Partial: row limit reached or hour not fully read", "部分：达到行数上限或未读完")}</span>
                <span><i aria-hidden="true" className="cov-swatch cov-cell cov-cell--gap" />{text("Not collected: not zero executions", "未采集：不是 0 次执行")}</span>
              </div>
            </div>

            <div className="collection-subblock">
              <div className="field-label collection-runs-title">{text("Recent reads", "最近读取记录")}</div>
              {status.runs.length === 0 ? (
                <p className="card-subtitle">{text("No reads yet.", "尚无读取记录。")}</p>
              ) : (
                <table aria-label={text("Recent collection reads", "最近采集读取记录")} className="data-table">
                  <thead>
                    <tr>
                      <th scope="col">{text("Started (UTC)", "时间 (UTC)")}</th>
                      <th scope="col">{text("Slice", "时间片")}</th>
                      <th className="num" scope="col">{text("Rows read", "读取行数")}</th>
                      <th className="num" scope="col">{text("Duration", "耗时")}</th>
                      <th scope="col">{text("Result", "结果")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {status.runs.map((run) => (
                      <tr key={`${run.slice_start_seconds}-${run.started_seconds}-${run.state}`}>
                        <td className="mono">{clockText(run.started_seconds).slice(11, 19)}</td>
                        <td className="mono">{clockText(run.slice_start_seconds).slice(11, 16)}–{clockText(run.slice_end_seconds).slice(11, 16)}</td>
                        <td className="num">{run.state === "gap" ? "—" : countText(run.rows_read) ?? run.rows_read}</td>
                        <td className="num"><span className="u-nowrap">{run.duration_ms}&nbsp;ms</span></td>
                        <td>{runBadge(run, text)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>
          </div>
        </Card>

        {/* Card 3 (col-3): Local history DB with fixed-footer Clear history… */}
        <Card
          actions={
            store.state === "available" ? (
              <span className="status-inline">
                <span aria-hidden="true" className="status-dot status-dot--ok" />
                {text("Available", "可用")}
              </span>
            ) : (
              <span className="state-stamp">{store.state === "disabled" ? text("Off", "已关闭") : text("Unavailable", "不可用")}</span>
            )
          }
          footer={
            store.state === "available" ? (
              <>
                <span>{text("Cluster data untouched", "不影响集群原数据")}</span>
                <Button
                  disabled={busy}
                  onClick={() => setConfirmOpen(true)}
                  size="sm"
                  variant="danger-outline"
                >
                  {text("Clear history…", "清空历史…")}
                </Button>
              </>
            ) : undefined
          }
          span={{ lg: 3, md: 8, sm: 4 }}
          title={text("Local history store", "本机历史库")}
          titleExtra={
            <InfoTip label={text("History store persistence boundary", "历史库持久化边界")}>
              {text(
                "Keeps: literal-free templates and fingerprints, hourly aggregates, governance snapshots, load error logs (30 days / 50 MB) and watermarks. Never keeps: SQL literals, Profile text. StarRocks passwords are not kept unless you tick saving.",
                "保存：脱敏模板与指纹、小时桶聚合、治理快照、导入错误日志（30 天 / 50 MB）与采集水位。不保存：原始 SQL 字面量与原始 Profile 文本；口令仅在显式勾选保存时以 AES-256-GCM 密文存储。",
              )}
            </InfoTip>
          }
          variant="form"
        >
          {store.state === "available" ? (
            <div className="collection-card-stack">
              <dl className="kv-grid">
                <dt>{text("Retention", "保留期")}</dt>
                <dd>{text(`${store.retention_days} days, expired data deleted automatically`, `${store.retention_days} 天自动过期`)}</dd>
                <dt>{text("Size", "当前占用")}</dt>
                <dd className="mono"><span className="u-nowrap">{formatHistoryBytes(store.file_bytes)}</span></dd>
                <dt>{text("Templates", "脱敏模板")}</dt>
                <dd className="mono"><AnimatedFigure value={countText(store.templates) ?? store.templates} /></dd>
                <dt>{text("Hourly buckets", "小时桶")}</dt>
                <dd className="mono"><AnimatedFigure value={countText(store.buckets) ?? store.buckets} /></dd>
              </dl>

              <p className="card-subtitle">
                {text(
                  "Never keeps: SQL literals, Profile text. StarRocks passwords are not kept unless you tick saving.",
                  "默认不存 SQL 字面量与 Profile 原文；仅在勾选保存时加密存储 1 个连接口令。",
                )}
              </p>

              <div className="notice-banner notice-banner--danger" role="note">
                <div>
                  <strong>{text("Clearing history deletes:", "清空历史后果：")}</strong>{" "}
                  {text(
                    "All template trends, alerts, governance items and pinned snapshots, saved load error logs, and the encrypted saved connection (stopping background collection when no page is open).",
                    "将删除全部模板趋势、告警记录、治理快照、导入错误日志原文及已加密保存的连接口令（无页面会话时后台采集随之停止）。",
                  )}
                </div>
              </div>

              {clearConfirm}
            </div>
          ) : (
            <div className="notice-banner" role="note">
              <span>
                {store.state === "disabled"
                  ? text("History is off in this setup (DOCTOR_HISTORY=off); nothing is collected.", "此运行方式未开启历史（DOCTOR_HISTORY=off），不会采集。")
                  : text("The history file could not be opened; nothing is collected.", "无法打开历史文件，不会采集。")}
              </span>
            </div>
          )}
        </Card>
      </PageGrid>

      <Dialog
        footer={
          <>
            <Button onClick={() => setConfirmOpen(false)} variant="secondary">{text("Cancel", "取消")}</Button>
            <Button
              disabled={busy}
              onClick={() => {
                setConfirmOpen(false);
                onClear();
              }}
              variant="danger"
            >
              {text("Delete everything", "确认清空")}
            </Button>
          </>
        }
        onOpenChange={setConfirmOpen}
        open={confirmOpen}
        title={text("Delete all local history?", "确认清空本机历史库？")}
        tone="danger"
      >
        <div className="notice-banner notice-banner--danger" role="alert">
          <div>
            {text(
              "This deletes all collected history (templates, hourly buckets, load history and saved error logs, watermarks, reads), every alert, governance item with its record, and pinned baseline snapshot, and the saved StarRocks connection (its background collection stops). It cannot be undone. StarRocks is not touched.",
              "将删除全部采集历史（模板、小时桶、导入历史与已存错误日志、采集水位、读取记录），以及全部告警、治理项及其处理记录和钉住的基线快照，以及已保存的 StarRocks 连接（后台采集随之停止），且无法恢复；不会改动 StarRocks 集群数据。",
            )}
          </div>
        </div>
      </Dialog>
    </>
  );
}
