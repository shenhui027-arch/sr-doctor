// Design board 06 (治理队列): a single-user record of what was changed outside
// this tool and whether the history shows it helped. The server owns the
// state machine: the user moves only along `allowed_transitions`, records a
// change to reach "pending verification", and only the comparator closes.
import { useEffect, useState, type JSX, type ReactNode } from "react";
import type {
  GovernanceItem,
  GovernanceListResponse,
  GovernanceState,
  VerificationOutcome,
} from "../../../../contracts/generated/doctor-contracts";
import { Button, Card, Dialog, EvidenceMeter, StateStamp, type EvidenceLevel } from "../../design";
import { localizedEnum, lookupEnumLabel } from "../../i18n/enum-label";
import type { LocalizedText } from "../../i18n/locale";
import { AuditMetricValue } from "../audit/AuditMetricValue";
import { clockText } from "./CollectionPanel";
import { trendChangeText } from "./TrendsPanel";
import "./alerts.css";

const STEPS: GovernanceState[] = ["new", "triaged", "in_progress", "pending_verification", "closed"];

const GOVERNANCE_STATE_LABELS: Record<GovernanceState, readonly [string, string]> = {
  new: ["New", "新建"],
  triaged: ["Triaged", "已分诊"],
  in_progress: ["In progress", "处理中"],
  pending_verification: ["Pending verification", "待验证"],
  closed: ["Closed · verified improved", "已关闭 · 验证改善"],
  wont_fix: ["Won't fix", "不处理"],
};

const ORIGIN_LABELS: Record<GovernanceItem["origin"], readonly [string, string]> = {
  alert: ["Alert", "告警"],
  trends: ["Regression list", "回归列表"],
  manual: ["Manual", "手动"],
};

const METRIC_LABELS: Record<string, readonly [string, string]> = {
  query_time: ["Median query time", "查询耗时中位数"],
  scan_bytes: ["Median scan bytes", "扫描数据量中位数"],
  cpu_cost: ["Median CPU", "CPU 中位数"],
};

const LEVEL_LABELS: Record<string, readonly [string, string]> = {
  high: ["high", "高"],
  medium: ["medium", "中"],
  low: ["low", "低"],
};

const EVIDENCE_LABELS: Record<string, readonly [string, string]> = {
  complete: ["complete", "完整"],
  partial: ["partial", "部分"],
  unavailable: ["unavailable", "不可用"],
};

export function governanceStateText(state: GovernanceState, text: LocalizedText): string {
  return lookupEnumLabel(GOVERNANCE_STATE_LABELS, state, text);
}

export function outcomeText(outcome: VerificationOutcome, text: LocalizedText): string {
  switch (outcome.state) {
    case "improved":
      return text("Verified · improved", "已验证 · 改善");
    case "not_improved":
      return text("Not improved", "未改善");
    case "insufficient_samples":
      return text("Collecting samples · no verdict yet", "样本积累中 · 暂不下结论");
    case "workload_signal_missing":
      return text("No scan or CPU signal · no verdict", "无扫描量或 CPU 信号 · 不下结论");
    case "not_comparable":
      if (outcome.reason === "engine_version_changed") {
        return text("StarRocks version changed · not comparable", "版本变化，暂不可比");
      }
      if (outcome.reason === "rule_version_changed") {
        return text("Rule set changed · not comparable", "规则集变化，暂不可比");
      }
      return `${text("Not comparable", "暂不可比")} (${localizedEnum(outcome.reason, text)})`;
    default:
      return localizedEnum(String((outcome as { state?: unknown }).state), text);
  }
}

export function originText(item: GovernanceItem, text: LocalizedText): string {
  if (item.origin === "alert") {
    return text(`Alert #${item.alert_id ?? ""}`, `告警 #${item.alert_id ?? ""}`);
  }
  return lookupEnumLabel(ORIGIN_LABELS, item.origin, text);
}

export function metricName(name: string, text: LocalizedText): string {
  return lookupEnumLabel(METRIC_LABELS, name, text);
}

function transitionButtonLabel(to: GovernanceState, currentState: GovernanceState, text: LocalizedText): string {
  switch (to) {
    case "triaged":
      return text("Mark triaged", "标为已分诊");
    case "in_progress":
      return currentState === "new" || currentState === "triaged"
        ? text("Start work", "开始处理")
        : text("Back to work", "退回处理中");
    case "wont_fix":
      return text("Won't fix", "不处理");
    default:
      return governanceStateText(to, text);
  }
}

function stateDotClass(state: GovernanceState): string {
  if (state === "closed") return "status-dot status-dot--ok";
  if (state === "pending_verification" || state === "in_progress") return "status-dot status-dot--warn";
  if (state === "new") return "status-dot status-dot--critical";
  return "status-dot status-dot--hollow";
}

function toMeterLevel(evidence: string): EvidenceLevel {
  if (evidence === "complete") return "complete";
  if (evidence === "partial") return "partial";
  return "missing";
}

export type GovernanceCommand =
  | { kind: "transition"; to: GovernanceState }
  | { kind: "pin" }
  | { kind: "change"; note: string }
  | { kind: "verify" };

/** Change-note drafts, one per item: a draft never moves to another item. */
export type NoteDrafts = Readonly<Record<string, string>>;

export function GovernancePanel({
  governance,
  busy,
  selectedId,
  onSelect,
  onCommand,
  onOpenTrends,
  notes = {},
  onNote = () => undefined,
  openItemId = null,
  onOpenedItem,
  toolbarLeading,
  toolbar,
  text,
}: {
  governance: GovernanceListResponse;
  busy: boolean;
  selectedId: string | null;
  onSelect(id: string): void;
  onCommand(item: GovernanceItem, command: GovernanceCommand): void;
  onOpenTrends(templateKey: string): void;
  notes?: NoteDrafts;
  onNote?(itemId: string, note: string): void;
  /** One-shot request (from an alert) to open this item's drawer once it is listed. */
  openItemId?: string | null;
  onOpenedItem?(): void;
  /** View switch (Alerts vs Governance) at the start of the unified toolbar. */
  toolbarLeading?: ReactNode;
  /** Target and refresh controls at the end of the unified toolbar. */
  toolbar?: ReactNode;
  text: LocalizedText;
}): JSX.Element {
  const items = governance.items;
  const selected = items.find((item) => item.item_id === selectedId) ?? items[0] ?? null;
  const requestListed = openItemId !== null && items.some((item) => item.item_id === openItemId);
  const [drawerOpen, setDrawerOpen] = useState(requestListed);
  useEffect(() => {
    if (!openItemId || !requestListed) return;
    onSelect(openItemId);
    setDrawerOpen(true);
    onOpenedItem?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [openItemId, requestListed]);

  return (
    <div className="alert-stage-view">
      <div className="workspace-bar">
        {toolbarLeading}
        <span className="muted">
          {text(
            "After you optimize in the cluster, record the change here and verify its effect",
            "在集群完成优化后，在此记录改动并验证效果",
          )}
        </span>
        {toolbar && <div className="workspace-bar-right">{toolbar}</div>}
      </div>

      <Card
        aria-labelledby="governance-list"
        footer={
          <span>
            {text(
              "Closed automatically only when verification confirms improvement",
              "仅验证确认改善后自动关闭",
            )}
          </span>
        }
        title={<span id="governance-list">{text("Governance queue", "治理队列")}</span>}
        variant="table"
      >
        {items.length === 0 ? (
          <div className="state-box">{text("No item yet. Add one from an alert or the regression list.", "还没有治理项。可从告警或回归列表加入。")}</div>
        ) : (
          <table className="data-table">
            <thead>
              <tr>
                <th scope="col">{text("Item", "治理项")}</th>
                <th scope="col">{text("Origin", "来源")}</th>
                <th scope="col">{text("Baseline snapshot", "基线快照")}</th>
                <th scope="col">{text("State", "状态")}</th>
              </tr>
            </thead>
            <tbody>
              {items.map((item) => {
                const isSelected = item.item_id === selected?.item_id;
                return (
                  <tr
                    aria-selected={isSelected}
                    className={isSelected ? "is-selected" : undefined}
                    key={item.item_id}
                    onClick={() => {
                      onSelect(item.item_id);
                      setDrawerOpen(true);
                    }}
                  >
                    <td>
                      <button
                        aria-pressed={isSelected}
                        className="row-pick-btn"
                        onClick={(event) => {
                          event.stopPropagation();
                          onSelect(item.item_id);
                          setDrawerOpen(true);
                        }}
                        type="button"
                      >
                        {item.title}
                      </button>
                      <div className="card-subtitle mono">{item.template ?? item.template_key}</div>
                    </td>
                    <td>{originText(item, text)}</td>
                    <td>
                      {item.snapshot ? (
                        <span className="status-inline mono">
                          <span className="status-dot status-dot--ok" />
                          {text(
                            `Pinned · ${clockText(item.snapshot.pinned_seconds).slice(5, 16)}`,
                            `已钉住 · ${clockText(item.snapshot.pinned_seconds).slice(5, 16)}`,
                          )}
                        </span>
                      ) : (
                        <span className="state-stamp" data-honest-state="missing">
                          ∅ {text("Not pinned", "未钉住")}
                        </span>
                      )}
                    </td>
                    <td>
                      <span className="status-inline">
                        <span className={stateDotClass(item.state)} />
                        {governanceStateText(item.state, text)}
                      </span>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </Card>

      <GovernanceItemDrawer
        busy={busy}
        item={selected}
        notes={notes}
        onCommand={onCommand}
        onNote={onNote}
        onOpenChange={(open) => {
          if (!open) setDrawerOpen(false);
        }}
        onOpenTrends={onOpenTrends}
        open={drawerOpen && selected !== null}
        text={text}
      />
    </div>
  );
}

export function GovernanceItemDrawer({
  busy,
  item,
  notes = {},
  onCommand,
  onNote = () => undefined,
  onOpenChange = () => undefined,
  onOpenTrends,
  open,
  text,
}: {
  busy: boolean;
  item: GovernanceItem | null;
  notes?: NoteDrafts;
  onCommand(item: GovernanceItem, command: GovernanceCommand): void;
  onNote?(itemId: string, note: string): void;
  onOpenChange?(open: boolean): void;
  onOpenTrends(templateKey: string): void;
  open: boolean;
  text: LocalizedText;
}): JSX.Element {
  const activeItem = open ? item : null;
  const stage = activeItem ? STEPS.indexOf(activeItem.state) : -1;
  const note = activeItem ? notes[activeItem.item_id] ?? "" : "";

  return (
    <Dialog
      footer={
        activeItem ? (
          <div className="alert-footer-actions">
            <div className="alert-footer-group">
              {activeItem.state === "pending_verification" && (
                <Button disabled={busy} onClick={() => onCommand(activeItem, { kind: "verify" })} size="sm" variant="primary">
                  {text("Verify the effect", "验证优化效果")}
                </Button>
              )}
              {(activeItem.state === "new" || activeItem.state === "triaged" || activeItem.state === "in_progress") && (
                <Button disabled={busy} onClick={() => onCommand(activeItem, { kind: "pin" })} size="sm" variant="secondary">
                  {activeItem.snapshot ? text("Re-pin baseline", "更换基线") : text("Pin baseline", "钉住基线")}
                </Button>
              )}
              {activeItem.allowed_transitions.map((to) => (
                <Button
                  disabled={busy}
                  key={to}
                  onClick={() => onCommand(activeItem, { kind: "transition", to })}
                  size="sm"
                  variant={to === "wont_fix" ? "secondary" : "primary"}
                >
                  {transitionButtonLabel(to, activeItem.state, text)}
                </Button>
              ))}
              <Button onClick={() => onOpenTrends(activeItem.template_key)} size="sm" variant="ghost">
                {text("View trend", "查看趋势")}
              </Button>
            </div>
            <span className="muted">
              {text(
                "An item closes only when verification shows an improvement; it cannot be closed by hand.",
                "仅验证确认改善后自动关闭；治理项不能手动关闭。",
              )}
            </span>
          </div>
        ) : undefined
      }
      onOpenChange={onOpenChange}
      open={activeItem !== null}
      placement="right"
      title={activeItem ? activeItem.title : ""}
      width="lg"
    >
      {activeItem ? (
        <div className="detail-stack" key={activeItem.item_id}>
          <div className="card-subtitle">{`${text("Item", "治理项")} #${activeItem.item_id}`}</div>

          <ol aria-label={text("Progress", "进度")} className="gov-steps">
            {STEPS.map((step, index) => (
              <li
                className={`gov-step${
                  activeItem.state === "wont_fix"
                    ? " is-skipped"
                    : index < stage
                      ? " is-done"
                      : index === stage
                        ? " is-current"
                        : ""
                }`}
                key={step}
              >
                {governanceStateText(step, text).split(" · ")[0]}
              </li>
            ))}
          </ol>
          {activeItem.state === "wont_fix" && (
            <span className="state-stamp">{governanceStateText("wont_fix", text)}</span>
          )}

          <h3 className="detail-sec-title">{text("Pinned baseline snapshot", "钉住的基线快照")}</h3>
          {activeItem.snapshot ? (
            <dl className="kv-grid">
              <dt>{text("Pinned (UTC)", "钉住时间（UTC）")}</dt>
              <dd className="mono">{clockText(activeItem.snapshot.pinned_seconds)}</dd>
              <dt>{text("Window", "窗口")}</dt>
              <dd className="mono">
                {clockText(activeItem.snapshot.window_start_seconds).slice(5, 16)} –{" "}
                {clockText(activeItem.snapshot.window_end_seconds).slice(5, 16)}
              </dd>
              <dt>{text("Median", "中位数")}</dt>
              <dd className="mono">
                <AuditMetricValue text={text} value={activeItem.snapshot.query_time_median} /> · {text("scan", "扫描")}{" "}
                <AuditMetricValue text={text} value={activeItem.snapshot.scan_bytes_median} /> · CPU{" "}
                <AuditMetricValue text={text} value={activeItem.snapshot.cpu_cost_median} />
              </dd>
              <dt>{text("Executions", "执行次数")}</dt>
              <dd className="mono">
                {activeItem.snapshot.executions} · {text("valid", "有效")} {activeItem.snapshot.valid_samples}
              </dd>
              <dt>{text("Representative", "代表执行")}</dt>
              <dd className="mono">{activeItem.snapshot.exemplar_query_id ?? <StateStamp state="missing" />}</dd>
            </dl>
          ) : (
            <div className="notice-banner notice-banner--warn">
              {text(
                "No snapshot pinned yet. Before recording a change, pin the template's recent executions as the baseline.",
                "尚未钉住基线快照。记录改动前，请先钉住该模板最近的执行作为基线。",
              )}
            </div>
          )}

          <h3 className="detail-sec-title">{text("Record", "处理记录")}</h3>
          <ol className="gov-events">
            {activeItem.events.map((event, index) => {
              const eventLabel =
                event.kind === "created"
                  ? text("Created", "创建")
                  : event.kind === "pinned"
                    ? text("Baseline pinned", "钉住基线")
                    : event.kind === "change_recorded"
                      ? text("Change recorded", "记录改动")
                      : event.kind === "verified"
                        ? text("Effect verified", "效果验证")
                        : event.kind === "state_changed"
                          ? `${event.from ? governanceStateText(event.from, text) : ""} → ${governanceStateText(event.to, text)}`
                          : localizedEnum(event.kind, text);
              return (
                <li className="gov-event" key={`${event.at_seconds}-${index}`}>
                  <span className="muted mono">{clockText(event.at_seconds).slice(5, 16)}</span>
                  <strong>{eventLabel}</strong>
                  <span className="muted">
                    {event.actor === "comparator"
                      ? text("automatic verification", "自动验证")
                      : event.actor === "local_user"
                        ? text("you", "你")
                        : localizedEnum(event.actor, text)}
                  </span>
                  {event.note && <span className="gov-event-note">{event.note}</span>}
                </li>
              );
            })}
          </ol>

          {activeItem.state === "in_progress" && (
            <div className="governance-change">
              <label className="field-label" htmlFor="governance-note">
                {text("What did you change outside this tool? (no SQL text)", "你在本工具外做了什么改动？（不要粘贴 SQL）")}
              </label>
              <textarea
                className="textarea"
                id="governance-note"
                maxLength={500}
                onChange={(event) => onNote(activeItem.item_id, event.target.value)}
                rows={2}
                value={note}
              />
              <div className="alert-footer-group">
                <Button
                  disabled={busy || !note.trim() || !activeItem.snapshot}
                  onClick={() => onCommand(activeItem, { kind: "change", note })}
                  size="sm"
                  variant="primary"
                >
                  {text("Record the change and wait for verification", "记录改动并进入待验证")}
                </Button>
                {!activeItem.snapshot && (
                  <span className="state-stamp state-stamp--warn">{text("Pin a baseline first.", "请先钉住基线。")}</span>
                )}
              </div>
            </div>
          )}

          <h3 className="detail-sec-title">{text("Before / after verification", "前后对比验证")}</h3>
          {activeItem.verification ? (
            <>
              <p className="muted mono">
                {text(
                  `Before: pinned window · after: ${clockText(activeItem.verification.after_start_seconds).slice(5, 16)} – ${clockText(activeItem.verification.after_end_seconds).slice(5, 16)}`,
                  `前：钉住的窗口 · 后：${clockText(activeItem.verification.after_start_seconds).slice(5, 16)} – ${clockText(activeItem.verification.after_end_seconds).slice(5, 16)}`,
                )}
              </p>
              <table className="data-table">
                <thead>
                  <tr>
                    <th scope="col">{text("Metric", "指标")}</th>
                    <th className="num" scope="col">{text("Before", "治理前")}</th>
                    <th className="num" scope="col">{text("After", "治理后")}</th>
                    <th className="num" scope="col">{text("Change", "变化")}</th>
                  </tr>
                </thead>
                <tbody>
                  {activeItem.verification.metrics.map((metric) => (
                    <tr key={metric.metric}>
                      <td>{metricName(metric.metric, text)}</td>
                      <td className="num"><AuditMetricValue text={text} value={metric.before} /></td>
                      <td className="num"><AuditMetricValue text={text} value={metric.after} /></td>
                      <td className={`num ${metric.change?.direction === "down" ? "delta-down" : metric.change?.direction === "up" ? "delta-up" : ""}`}>
                        {trendChangeText(metric.change)}
                      </td>
                    </tr>
                  ))}
                  <tr>
                    <td>{text("Executions", "执行次数")}</td>
                    <td className="num">{activeItem.verification.executions_before}</td>
                    <td className="num">{activeItem.verification.executions_after}</td>
                    <td className="num">—</td>
                  </tr>
                </tbody>
              </table>
              <div className="fact-pills">
                <span className="status-inline">
                  <span
                    className={
                      activeItem.verification.outcome.state === "improved"
                        ? "status-dot status-dot--ok"
                        : activeItem.verification.outcome.state === "not_improved"
                          ? "status-dot status-dot--critical"
                          : "status-dot status-dot--hollow"
                    }
                  />
                  {outcomeText(activeItem.verification.outcome, text)}
                </span>
                <span>
                  {text("Confidence", "置信度")}{" "}
                  {activeItem.verification.confidence ? lookupEnumLabel(LEVEL_LABELS, activeItem.verification.confidence, text) : "—"}
                </span>
                <EvidenceMeter
                  label={`${text("Evidence", "证据")} ${lookupEnumLabel(EVIDENCE_LABELS, activeItem.verification.evidence, text)}`}
                  level={toMeterLevel(activeItem.verification.evidence)}
                />
              </div>
            </>
          ) : (
            <p className="muted">
              {activeItem.change_seconds !== null
                ? text(
                    `After window starts at the first whole hour after ${clockText(activeItem.change_seconds)}.`,
                    `治理后窗口从 ${clockText(activeItem.change_seconds)} 之后的第一个整点开始累计。`,
                  )
                : text("No after window yet: record the change first.", "尚无治理后窗口：请先记录改动。")}
            </p>
          )}
        </div>
      ) : null}
    </Dialog>
  );
}
