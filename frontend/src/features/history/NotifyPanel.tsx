// Design board 08-settings: outbound notification channels, their targets,
// the shared sending rules and the delivery log. Two-card responsive grid
// (col-5 channels & rules | col-7 targets & delivery log).
import { useState } from "react";
import type { JSX, ReactNode } from "react";
import type {
  NotifyChannelKind,
  NotifyDeliveriesResponse,
  NotifyDeliveryState,
  NotifyLanguage,
  NotifyOverviewResponse,
  NotifySettings,
  NotifyTargetView,
  NotifyTestResponse,
} from "../../../../contracts/generated/doctor-contracts";
import { Button, Card, InfoTip, PageGrid, SegTabs, type SegItem } from "../../design";
import type { LocalizedText } from "../../i18n/locale";
import { clockText } from "./history-status";
import {
  channelStatus,
  deliveryStateText,
  durationText,
  errorText,
  eventText,
  kindName,
  NOTIFY_KINDS,
  severityText,
  shortKindName,
  testText,
  type Tone,
} from "./notify-text";
import "./notify.css";

export type TestResults = Record<string, NotifyTestResponse | "running">;
export type NotifyRightTab = "targets" | "deliveries";

export function TestChip({ result, text }: { result: NotifyTestResponse | "running" | undefined; text: LocalizedText }): JSX.Element | null {
  if (!result) return null;
  if (result === "running") return <span className="state-stamp" role="status">{text("Sending test…", "正在发送测试…")}</span>;
  if (result.state === "sent") {
    return (
      <span className="status-inline" role="status">
        <span aria-hidden="true" className="status-dot status-dot--ok" />
        {testText(result, text)}
      </span>
    );
  }
  return <span className="state-stamp state-stamp--critical" role="status">{testText(result, text)}</span>;
}

function renderChannelStatus(status: { label: string; tone: Tone }): JSX.Element {
  if (status.tone === "ok") {
    return (
      <span className="status-inline">
        <span aria-hidden="true" className="status-dot status-dot--ok" />
        {status.label}
      </span>
    );
  }
  if (status.tone === "bad") {
    return <span className="state-stamp state-stamp--critical">{status.label}</span>;
  }
  if (status.tone === "warn") {
    return <span className="state-stamp state-stamp--warn">{status.label}</span>;
  }
  return <span className="state-stamp">{status.label}</span>;
}

function lastText(target: NotifyTargetView, text: LocalizedText): { label: string; critical: boolean } {
  const last = target.last;
  if (!last) return { label: "—", critical: false };
  const when = clockText(last.at_seconds).slice(5, 16);
  const why = last.error ? ` · ${errorText(last.error, text)}${last.provider_code ? ` ${last.provider_code}` : ""}` : "";
  return {
    label: `${deliveryStateText(last.state, text)} · ${when}${why}`,
    critical: last.state === "failed",
  };
}

export function NotifyPanel({
  overview,
  busy,
  tests,
  onOpen,
  onTest,
  onSaveSettings,
  deliveriesView,
  rightTab = "targets",
  onRightTabChange,
  text,
}: {
  overview: NotifyOverviewResponse;
  busy: boolean;
  tests: TestResults;
  onOpen(kind: NotifyChannelKind, targetId?: string): void;
  onTest(target: NotifyTargetView): void;
  onSaveSettings(settings: NotifySettings): void;
  deliveriesView?: ReactNode;
  rightTab?: NotifyRightTab;
  onRightTabChange?(tab: NotifyRightTab): void;
  text: LocalizedText;
}): JSX.Element {
  const [filter, setFilter] = useState<NotifyChannelKind | "all">("all");
  const [search, setSearch] = useState("");
  const [draft, setDraft] = useState<NotifySettings>(overview.settings);
  const needle = search.trim().toLowerCase();
  const rows = overview.targets.filter(
    (target) =>
      (filter === "all" || target.kind === filter)
      && (!needle || target.name.toLowerCase().includes(needle) || target.address_hint.toLowerCase().includes(needle)),
  );
  const enabledCount = overview.targets.filter((target) => target.enabled).length;

  const rightTabs: readonly SegItem<NotifyRightTab>[] = [
    { value: "targets", label: text("Targets", "发送对象"), count: overview.targets.length },
    { value: "deliveries", label: text("Delivery log (24h)", "投递记录与短信配额 (24h)") },
  ];

  const showingDeliveries = Boolean(deliveriesView && rightTab === "deliveries");

  return (
    <PageGrid className="notify-grid-stage">
      <Card
        footer={
          <>
            <span>{text("Secrets encrypted with AES-256-GCM · write-only", "主密钥加密（AES-256-GCM 只写不读）")}</span>
            <Button
              disabled={busy}
              onClick={() => onSaveSettings({ ...draft, public_url: draft.public_url?.trim() ? draft.public_url.trim() : null })}
              size="sm"
              variant="primary"
            >
              {text("Save rules", "保存发送规则")}
            </Button>
          </>
        }
        span={{ lg: 5, md: 8, sm: 4 }}
        title={text("Channels and sending rules", "通知渠道与共用发送规则")}
        titleExtra={
          <InfoTip label={text("Channels and targets", "渠道与对象关系说明")}>
            {text(
              "A channel defines server or cloud credentials (how to send); a target defines a mailbox, group bot or phone number (to whom), up to 20 targets per channel. Messages carry cluster, template fingerprint, severity, confidence and median change — never SQL literals, Profiles or passwords.",
              "渠道定义服务器或云平台凭据（怎么发）；发送对象定义邮箱、群机器人或手机号（发给谁），每个渠道最多 20 个对象。消息只含集群、模板指纹、严重度、置信度与中位数变化，绝不含 SQL 字面量、Profile 或口令。",
            )}
          </InfoTip>
        }
        variant="form"
      >
        <div className="notify-left-body">
          {!overview.remote_collection_allowed ? (
            <div className="notice-banner notice-banner--warn" role="status">
              <span>
                {text(
                  "Remote clusters are not collected until the G1 production read-only pilot passes (DOCTOR_COLLECTION_ALLOW_REMOTE is not set), so they raise no new alert and nothing is sent. Local and test clusters are not affected.",
                  "远端集群在 G1 生产只读试点通过前不做后台采集（DOCTOR_COLLECTION_ALLOW_REMOTE 未设置），此时不会产生新告警，也不会发出任何通知。本机 / 测试集群不受影响。",
                )}
              </span>
            </div>
          ) : null}

          <ul aria-label={text("Notification channels", "通知渠道列表")} className="channel-list">
            <li className="channel-item channel-item--static">
              <div className="channel-item-main">
                <strong>{text("In app", "应用内告警")}</strong>
                <span className="card-subtitle">{text("Top-bar badge and alert list · nothing to set up", "顶栏角标与告警列表 · 无需配置")}</span>
              </div>
              <span className="status-inline">
                <span aria-hidden="true" className="status-dot status-dot--ok" />
                {text("Always on", "始终开启")}
              </span>
            </li>

            {NOTIFY_KINDS.map((kind) => {
              const channel = overview.channels.find((item) => item.kind === kind);
              if (!channel) return null;
              const status = channelStatus(channel, overview.targets, text);
              return (
                <li key={kind}>
                  <button aria-haspopup="dialog" className="channel-item" onClick={() => onOpen(kind)} type="button">
                    <div className="channel-item-main">
                      <strong>{kindName(kind, text)}</strong>
                      <span className="card-subtitle">
                        {channel.targets === 0
                          ? text("No target yet", "还没有发送对象")
                          : text(`${channel.targets} targets · ${channel.enabled_targets} on`, `${channel.targets} 个发送对象 · 启用 ${channel.enabled_targets}`)}
                      </span>
                    </div>
                    <span className="channel-item-right">
                      {renderChannelStatus(status)}
                      {kind !== "email" ? <span className="state-stamp" title={text("Real provider verification pending", "真实平台发送待验证")}>{text("Pending verify", "待验证")}</span> : null}
                      <span className="card-subtitle">{channel.targets === 0 && status.tone === "off" ? text("Set up", "设置") : text("Edit", "编辑")} ›</span>
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>

          <div className="notify-rules-section">
            <span className="field-label">{text("Shared sending rules", "共用发送规则")}</span>
            <label className="check-inline">
              <input checked disabled type="checkbox" />
              <span>{text("New alerts (including ones still regressing when a mute ends; no repeat sends)", "新告警必发（含静默期满仍在回归，持续回归不重复发）")}</span>
            </label>
            <label className="check-inline">
              <input checked={draft.notify_resolved} onChange={(event) => setDraft({ ...draft, notify_resolved: event.target.checked })} type="checkbox" />
              <span>{text("Alerts back inside the baseline band (resolved)", "告警恢复到基线带时发送（已解决）")}</span>
            </label>
            <label className="check-inline">
              <input checked={draft.include_template} onChange={(event) => setDraft({ ...draft, include_template: event.target.checked })} type="checkbox" />
              <span>{text("Add the literal-free SQL template (never in SMS)", "附带脱敏 SQL 模板（短信始终不附带；绝不含字面量、Profile 或口令）")}</span>
            </label>

            <div className="form-grid-2">
              <div className="field">
                <label className="field-label" htmlFor="notify-rule-lang">{text("Language", "通知语言")}</label>
                <select
                  className="select"
                  id="notify-rule-lang"
                  onChange={(event) => setDraft({ ...draft, language: event.target.value as NotifyLanguage })}
                  value={draft.language}
                >
                  <option value="zh">中文</option>
                  <option value="en">English</option>
                </select>
              </div>
              <div className="field">
                <label className="field-label" htmlFor="notify-rule-url">{text("External URL (optional)", "外部访问地址（选填）")}</label>
                <input
                  className="input mono"
                  id="notify-rule-url"
                  onChange={(event) => setDraft({ ...draft, public_url: event.target.value })}
                  placeholder="http://10.0.0.5:8080"
                  type="url"
                  value={draft.public_url ?? ""}
                />
              </div>
            </div>
          </div>
        </div>
      </Card>

      <Card
        actions={
          showingDeliveries ? null : (
            <>
              <span className="card-subtitle">
                {text(`${overview.targets.length} in all · ${enabledCount} on`, `共 ${overview.targets.length} 个 · 启用 ${enabledCount} 个`)}
              </span>
              <div className="filter-chip-input">
                <label htmlFor="notify-target-channel-filter">{text("Channel", "渠道")}</label>
                <select
                  id="notify-target-channel-filter"
                  onChange={(event) => setFilter(event.target.value as NotifyChannelKind | "all")}
                  value={filter}
                >
                  <option value="all">{text("All", "全部")}</option>
                  {NOTIFY_KINDS.map((kind) => <option key={kind} value={kind}>{shortKindName(kind, text)}</option>)}
                </select>
              </div>
              <div className="filter-chip-input">
                <label htmlFor="notify-target-search">{text("Search", "搜索")}</label>
                <input
                  id="notify-target-search"
                  onChange={(event) => setSearch(event.target.value)}
                  placeholder={text("Name or address", "名称或地址")}
                  size={12}
                  value={search}
                />
              </div>
            </>
          )
        }
        footer={
          <>
            <span>{text("Select any channel on the left to add or edit targets · masked addresses only", "在左侧选择任一渠道即可新增或编辑对象 · 地址仅脱敏展示，密钥不回显")}</span>
            <span className="mono u-nowrap">{text("Timeout 10\u00A0s · max 4 attempts", "超时 10\u00A0s · 最多重试 4 次")}</span>
          </>
        }
        span={{ lg: 7, md: 8, sm: 4 }}
        title={
          onRightTabChange && deliveriesView ? (
            <SegTabs
              aria-label={text("Targets and delivery log", "发送对象与投递记录切换")}
              items={rightTabs}
              onValueChange={onRightTabChange}
              size="sm"
              value={rightTab}
            />
          ) : (
            text("Targets", "发送对象")
          )
        }
        variant="table"
      >
        {showingDeliveries ? (
          deliveriesView
        ) : overview.targets.length === 0 ? (
          <div className="state-box">
            <strong>{text("No target yet", "还没有发送对象")}</strong>
            <span>{text("Open a channel on the left to add one; a channel sends nothing without targets.", "打开左侧渠道添加；没有发送对象的渠道不会发出消息。")}</span>
          </div>
        ) : (
          <table aria-label={text("Notification targets", "外发通知发送对象表")} className="data-table">
            <thead>
              <tr>
                <th>{text("Name", "名称")}</th>
                <th>{text("Channel", "渠道")}</th>
                <th>{text("Address / number", "脱敏地址 / 号码 / Webhook")}</th>
                <th>{text("Receives", "接收严重度")}</th>
                <th>{text("State", "状态")}</th>
                <th>{text("Latest delivery", "最近投递")}</th>
                <th className="num">{text("Actions", "操作")}</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((target) => {
                const last = lastText(target, text);
                return (
                  <tr key={target.target_id}>
                    <td><strong>{target.name}</strong></td>
                    <td>{shortKindName(target.kind, text)}</td>
                    <td className="mono">{target.address_hint}</td>
                    <td>{severityText(target.min_severity, text)}</td>
                    <td>
                      {target.secret_unreadable ? (
                        <span className="state-stamp state-stamp--warn">{text("Re-enter secrets", "需重新填写密钥")}</span>
                      ) : target.enabled ? (
                        <span className="status-inline"><span aria-hidden="true" className="status-dot status-dot--ok" />{text("On", "启用")}</span>
                      ) : (
                        <span className="state-stamp">{text("Off", "停用")}</span>
                      )}
                    </td>
                    <td>
                      <span className={last.critical ? "mono delta-up" : "mono"}>{last.label}</span>{" "}
                      <TestChip result={tests[target.target_id]} text={text} />
                    </td>
                    <td className="num">
                      <span className="notify-table-actions">
                        <Button disabled={busy || tests[target.target_id] === "running"} onClick={() => onTest(target)} size="sm" variant="ghost">
                          {text("Test", "测试")}
                        </Button>
                        <Button aria-haspopup="dialog" onClick={() => onOpen(target.kind, target.target_id)} size="sm" variant="secondary">
                          {text("Edit", "编辑")}
                        </Button>
                      </span>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </Card>
    </PageGrid>
  );
}

export type DeliveryFilters = { kind: NotifyChannelKind | null; target_id: string | null; state: NotifyDeliveryState | null };

export function DeliveriesPanel({
  deliveries,
  targets,
  filters,
  onFilters,
  embedded = false,
  headerExtra,
  text,
}: {
  deliveries: NotifyDeliveriesResponse;
  targets: NotifyTargetView[];
  filters: DeliveryFilters;
  onFilters(filters: DeliveryFilters): void;
  embedded?: boolean;
  headerExtra?: ReactNode;
  text: LocalizedText;
}): JSX.Element {
  const sms = deliveries.sms_usage;

  const filterControls = (
    <div className="notify-deliveries-filter-bar">
      <div className="filter-chip-input">
        <label htmlFor="deliveries-channel-filter">{text("Channel", "渠道")}</label>
        <select
          id="deliveries-channel-filter"
          onChange={(event) => onFilters({ ...filters, kind: (event.target.value || null) as NotifyChannelKind | null })}
          value={filters.kind ?? ""}
        >
          <option value="">{text("All", "全部")}</option>
          {NOTIFY_KINDS.map((kind) => <option key={kind} value={kind}>{shortKindName(kind, text)}</option>)}
        </select>
      </div>
      <div className="filter-chip-input">
        <label htmlFor="deliveries-target-filter">{text("Target", "发送对象")}</label>
        <select
          id="deliveries-target-filter"
          onChange={(event) => onFilters({ ...filters, target_id: event.target.value || null })}
          value={filters.target_id ?? ""}
        >
          <option value="">{text("All", "全部")}</option>
          {targets.map((target) => <option key={target.target_id} value={target.target_id}>{target.name}</option>)}
        </select>
      </div>
      <div className="filter-chip-input">
        <label htmlFor="deliveries-state-filter">{text("Result", "结果")}</label>
        <select
          id="deliveries-state-filter"
          onChange={(event) => onFilters({ ...filters, state: (event.target.value || null) as NotifyDeliveryState | null })}
          value={filters.state ?? ""}
        >
          <option value="">{text("All", "全部")}</option>
          {(["sent", "failed", "pending", "skipped"] as NotifyDeliveryState[]).map((state) => (
            <option key={state} value={state}>{deliveryStateText(state, text)}</option>
          ))}
        </select>
      </div>
      <span className="card-subtitle">
        {text(
          `Kept ${deliveries.retention_days} days · only results and error categories, never message text or secrets`,
          `保留 ${deliveries.retention_days} 天 · 只记结果与错误类别，不记消息正文与密钥`,
        )}
      </span>
      {headerExtra}
    </div>
  );

  const content = (
    <>
      <div aria-label={text("24h delivery and SMS quota stats", "24 小时投递与短信配额统计")} className="stat-strip-4">
        <div className="stat-mini">
          <span className="card-subtitle">{text("Sent · 24h", "24h 已发送")}</span>
          <strong>{deliveries.sent_24h}</strong>
        </div>
        <div className="stat-mini">
          <span className="card-subtitle">{text("Failed (retries exhausted)", "失败（重试耗尽）")}</span>
          <strong className={deliveries.failed_24h > 0 ? "is-critical" : undefined}>{deliveries.failed_24h}</strong>
        </div>
        <div className="stat-mini">
          <span className="card-subtitle">{text("Waiting to retry", "等待重试")}</span>
          <strong>{deliveries.pending}</strong>
        </div>
        <div className="stat-mini">
          <span className="card-subtitle">{text("SMS this hour / limit", "短信本小时用量 / 上限")}</span>
          <strong>{sms.length === 0 ? "—" : sms.map((usage) => `${usage.used} / ${usage.hourly_limit}`).join(" · ")}</strong>
        </div>
      </div>

      {filterControls}

      {deliveries.deliveries.length === 0 ? (
        <div className="state-box">
          <strong>{text("Nothing sent yet", "还没有投递记录")}</strong>
        </div>
      ) : (
        <table aria-label={text("Delivery log", "最近通知投递日志表")} className="data-table">
          <thead>
            <tr>
              <th>{text("Time (UTC)", "时间 (UTC)")}</th>
              <th>{text("Channel · target", "渠道 · 发送对象")}</th>
              <th>{text("Content", "内容摘要")}</th>
              <th>{text("Result", "结果")}</th>
              <th className="num">{text("Attempts", "尝试")}</th>
              <th>{text("Category & code (no body)", "错误类别与返回短码（不含正文）")}</th>
            </tr>
          </thead>
          <tbody>
            {deliveries.deliveries.map((row) => {
              const notes = [
                row.error ? `${errorText(row.error, text)}${row.provider_code ? ` · ${row.provider_code}` : ""}` : null,
                row.next_attempt_seconds !== null ? text(`retry at ${clockText(row.next_attempt_seconds).slice(11, 16)}`, `${clockText(row.next_attempt_seconds).slice(11, 16)} 重试`) : null,
                row.duration_ms !== null && row.state !== "skipped" ? durationText(row.duration_ms) : null,
              ].filter((note): note is string => note !== null);
              return (
                <tr key={row.delivery_id}>
                  <td className="mono">{clockText(row.at_seconds).slice(5, 19)}</td>
                  <td>{shortKindName(row.kind, text)} · {row.target_name}</td>
                  <td>{eventText(row.event, row.alert_count, row.cluster, text)}</td>
                  <td>
                    {row.state === "sent" ? (
                      <span className="status-inline"><span aria-hidden="true" className="status-dot status-dot--ok" />{deliveryStateText(row.state, text)}</span>
                    ) : row.state === "failed" ? (
                      <span className="status-inline"><span aria-hidden="true" className="status-dot status-dot--critical" />{deliveryStateText(row.state, text)}</span>
                    ) : row.state === "pending" ? (
                      <span className="state-stamp state-stamp--warn">{deliveryStateText(row.state, text)}</span>
                    ) : (
                      <span className="state-stamp">{deliveryStateText(row.state, text)}</span>
                    )}
                  </td>
                  <td className="num">{row.attempts}</td>
                  <td className="mono">{notes.join(" · ") || "—"}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </>
  );

  if (embedded) return content;

  return (
    <Card
      footer={
        <>
          <span>{text("Each send times out after 10 s; network failures retry at 1, 5 and 30 min (4 attempts).", "每次发送超时 10 秒；网络类失败按 1 分钟、5 分钟、30 分钟重试，共 4 次。")}</span>
          <span>{text("SMS above the hourly limit is skipped; alerts stay in the app.", "短信超出渠道每小时上限时跳过并标记，告警仍保留在应用内。")}</span>
        </>
      }
      title={text("Delivery log", "投递记录")}
      variant="table"
    >
      {content}
    </Card>
  );
}
