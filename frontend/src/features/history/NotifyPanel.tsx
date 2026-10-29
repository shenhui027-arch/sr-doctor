// Design board N1 / N2 (v1.0.1): outbound notification channels, their
// targets, the shared sending rules and the delivery log. The server owns
// every state; this renders its DTOs and sends the user's edits back.
import { useState } from "react";
import type { JSX } from "react";
import type {
  NotifyChannelKind, NotifyDeliveriesResponse, NotifyDeliveryState, NotifyLanguage, NotifyOverviewResponse, NotifySettings,
  NotifyTargetView, NotifyTestResponse,
} from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { clockText } from "./history-status";
import {
  NOTIFY_KINDS, channelStatus, deliveryStateText, durationText, errorText, eventText, kindName, severityText, shortKindName, testText,
} from "./notify-text";
import "./notify.css";

export type TestResults = Record<string, NotifyTestResponse | "running">;

export function TestChip({ result, text }: { result: NotifyTestResponse | "running" | undefined; text: LocalizedText }): JSX.Element | null {
  if (!result) return null;
  if (result === "running") return <span className="notify-chip off" role="status">{text("Sending test…", "正在发送测试…")}</span>;
  return <span className={`notify-chip ${result.state === "sent" ? "ok" : "bad"}`} role="status">{testText(result, text)}</span>;
}

function lastText(target: NotifyTargetView, text: LocalizedText): { label: string; tone: string } {
  const last = target.last;
  if (!last) return { label: "—", tone: "muted" };
  const when = clockText(last.at_seconds).slice(5, 16);
  const why = last.error ? ` · ${errorText(last.error, text)}${last.provider_code ? ` ${last.provider_code}` : ""}` : "";
  return { label: `${deliveryStateText(last.state, text)} · ${when}${why}`, tone: last.state === "failed" ? "bad" : last.state === "sent" ? "plain" : "muted" };
}

export function NotifyPanel({ overview, busy, tests, onOpen, onTest, onSaveSettings, text }: {
  overview: NotifyOverviewResponse;
  busy: boolean;
  tests: TestResults;
  /** Opens the channel dialog, optionally on one target. */
  onOpen(kind: NotifyChannelKind, targetId?: string): void;
  onTest(target: NotifyTargetView): void;
  onSaveSettings(settings: NotifySettings): void;
  text: LocalizedText;
}): JSX.Element {
  const [filter, setFilter] = useState<NotifyChannelKind | "all">("all");
  const [search, setSearch] = useState("");
  const [draft, setDraft] = useState<NotifySettings>(overview.settings);
  const needle = search.trim().toLowerCase();
  const rows = overview.targets.filter((target) => (filter === "all" || target.kind === filter)
    && (!needle || target.name.toLowerCase().includes(needle) || target.address_hint.toLowerCase().includes(needle)));
  const enabledCount = overview.targets.filter((target) => target.enabled).length;
  return <div className="history-page notify-page">
    <p className="history-muted">{text("A channel is how to send (server, cloud account); a target is to whom (mailbox, group bot, phone). Each channel takes several targets.", "渠道 = 怎么发（服务器、平台凭据）；发送对象 = 发给谁（邮箱、群机器人、手机号），每个渠道可以有多个。")}</p>
    {!overview.remote_collection_allowed && <p className="history-banner" role="status">
      {text("Remote clusters are not collected until the G1 production read-only pilot passes (DOCTOR_COLLECTION_ALLOW_REMOTE is not set), so they raise no new alert and nothing is sent. Local and test clusters are not affected.", "远端集群在 G1 生产只读试点通过前不做后台采集（DOCTOR_COLLECTION_ALLOW_REMOTE 未设置），此时不会产生新告警，也不会发出任何通知。本机 / 测试集群不受影响。")}
    </p>}
    <section aria-labelledby="notify-channels" className="history-card">
      <div className="history-card-head">
        <h2 id="notify-channels">{text("Channels", "渠道")}</h2>
        <span className="history-muted">{text("Click a card to open its settings and manage its targets", "点击卡片打开设置弹窗，在弹窗里管理该渠道的发送对象")}</span>
      </div>
      <ul className="notify-cards">
        <li className="notify-card static">
          <span className="notify-card-head"><strong>{text("In app", "应用内")}</strong><span className="notify-chip ok">{text("Always on", "始终开启")}</span></span>
          <span className="history-muted">{text("Navigation badge and the alert list; nothing to set up", "导航角标与告警列表，无需设置")}</span>
        </li>
        {NOTIFY_KINDS.map((kind) => {
          const channel = overview.channels.find((item) => item.kind === kind);
          if (!channel) return null;
          const status = channelStatus(channel, overview.targets, text);
          return <li key={kind}>
            <button aria-haspopup="dialog" className="notify-card" onClick={() => onOpen(kind)} type="button">
              <span className="notify-card-head"><strong>{kindName(kind, text)}</strong><span className={`notify-chip ${status.tone}`}>{status.label}</span></span>
              <span className="notify-card-count">{channel.targets === 0 ? text("No target yet", "还没有发送对象") : text(`${channel.targets} targets · ${channel.enabled_targets} on`, `${channel.targets} 个发送对象 · 启用 ${channel.enabled_targets}`)}</span>
              <span className="notify-card-action">{channel.targets === 0 && status.tone === "off" ? text("Set up", "设置") : text("Edit", "编辑")} ›</span>
            </button>
          </li>;
        })}
      </ul>
    </section>
    <section aria-labelledby="notify-targets" className="history-card">
      <div className="history-card-head">
        <h2 id="notify-targets">{text("Targets", "发送对象")} <span className="history-muted">{text(`${overview.targets.length} in all · ${enabledCount} on`, `共 ${overview.targets.length} 个 · 启用 ${enabledCount} 个`)}</span></h2>
        <label className="notify-filter">{text("Channel", "渠道")}
          <select onChange={(event) => setFilter(event.target.value as NotifyChannelKind | "all")} value={filter}>
            <option value="all">{text("All", "全部")}</option>
            {NOTIFY_KINDS.map((kind) => <option key={kind} value={kind}>{shortKindName(kind, text)}</option>)}
          </select>
        </label>
        <label className="notify-filter">{text("Search", "搜索")}
          <input onChange={(event) => setSearch(event.target.value)} placeholder={text("Name or address", "名称或地址")} value={search} />
        </label>
      </div>
      {overview.targets.length === 0 ? <p className="notify-empty">{text("No target yet. Open a channel above and add one; a channel sends nothing without targets.", "还没有发送对象。打开上方的渠道添加；没有发送对象的渠道不会发出消息。")}</p> : <div className="history-table-wrap">
        <table className="history-table notify-table">
          <thead><tr>
            <th>{text("Name", "名称")}</th><th>{text("Channel", "渠道")}</th><th>{text("Address / number", "地址 / 号码")}</th>
            <th>{text("Receives", "接收严重度")}</th><th>{text("State", "状态")}</th><th>{text("Latest delivery", "最近投递")}</th><th className="num">{text("Actions", "操作")}</th>
          </tr></thead>
          <tbody>
            {rows.map((target) => {
              const last = lastText(target, text);
              return <tr key={target.target_id}>
                <td><strong>{target.name}</strong></td>
                <td>{shortKindName(target.kind, text)}</td>
                <td className="mono">{target.address_hint}</td>
                <td>{severityText(target.min_severity, text)}</td>
                <td>{target.secret_unreadable ? <span className="notify-chip warn">{text("Re-enter secrets", "需重新填写")}</span>
                  : <span className={`notify-chip ${target.enabled ? "ok" : "off"}`}>{target.enabled ? text("On", "启用") : text("Off", "停用")}</span>}</td>
                <td><span className={`notify-last ${last.tone}`}>{last.label}</span> <TestChip result={tests[target.target_id]} text={text} /></td>
                <td className="num">
                  <button className="button text" disabled={busy || tests[target.target_id] === "running"} onClick={() => onTest(target)} type="button">{text("Test", "测试")}</button>
                  <button aria-haspopup="dialog" className="button text" onClick={() => onOpen(target.kind, target.target_id)} type="button">{text("Edit", "编辑")}</button>
                </td>
              </tr>;
            })}
          </tbody>
        </table>
      </div>}
      <p className="history-muted">{text("Addresses and numbers are shown masked; webhook tokens and bot secrets are never shown. Edit opens the channel's dialog on that row.", "地址与号码只显示脱敏后的样子；Webhook 里的 token 和机器人密钥不回显。“编辑”打开对应渠道的弹窗并定位到这一行。")}</p>
    </section>
    <section aria-labelledby="notify-rules" className="history-card">
      <div className="history-card-head">
        <h2 id="notify-rules">{text("Sending rules", "发送规则")} <span className="history-muted">{text("shared by all channels", "所有渠道共用")}</span></h2>
        <button className="button accent" disabled={busy} onClick={() => onSaveSettings({ ...draft, public_url: draft.public_url?.trim() ? draft.public_url.trim() : null })} type="button">{text("Save rules", "保存规则")}</button>
      </div>
      <div className="notify-rules">
        <div className="notify-field-group">
          <span className="history-muted">{text("When", "触发")}</span>
          <label className="notify-check"><input checked disabled type="checkbox" /> {text("New alerts (including ones still regressing when a mute ends)", "新告警（含静默结束后仍在回归）")}</label>
          <label className="notify-check"><input checked={draft.notify_resolved} onChange={(event) => setDraft({ ...draft, notify_resolved: event.target.checked })} type="checkbox" /> {text("Alerts back inside the baseline band (resolved)", "告警恢复到基线带（已解决）")}</label>
        </div>
        <div className="notify-field-group">
          <span className="history-muted">{text("Content", "内容")}</span>
          <label className="notify-check"><input checked={draft.include_template} onChange={(event) => setDraft({ ...draft, include_template: event.target.checked })} type="checkbox" /> {text("Add the literal-free SQL template (never in SMS)", "附带脱敏 SQL 模板（短信始终不带）")}</label>
          <label className="notify-field inline">{text("Language", "语言")}
            <select onChange={(event) => setDraft({ ...draft, language: event.target.value as NotifyLanguage })} value={draft.language}>
              <option value="zh">中文</option><option value="en">English</option>
            </select>
          </label>
        </div>
        <label className="notify-field">{text("SR Doctor address (for the \"view alerts\" link; optional)", "SR Doctor 访问地址（消息里“查看告警”链接，可留空）")}
          <input className="mono" onChange={(event) => setDraft({ ...draft, public_url: event.target.value })} placeholder="http://10.0.0.5:8080" value={draft.public_url ?? ""} />
        </label>
      </div>
      <ul className="notify-notes">
        <li>{text("Alerts raised by one collection run go to each target as one message; muted alerts are not sent and a lasting regression is not sent again.", "同一次采集评估产生的多条告警，每个发送对象合并为一条消息；静默中的告警不发，持续回归不重复发。")}</li>
        <li>{text("Messages carry the cluster, template fingerprint, severity, confidence and median change; never SQL literals, Profiles or passwords. Receiving severity is set per target.", "消息只含集群、模板指纹、严重度、置信度与中位数变化；不含 SQL 字面量、Profile 或口令。接收严重度在每个发送对象上单独设置。")}</li>
      </ul>
    </section>
  </div>;
}

export type DeliveryFilters = { kind: NotifyChannelKind | null; target_id: string | null; state: NotifyDeliveryState | null };

export function DeliveriesPanel({ deliveries, targets, filters, onFilters, text }: {
  deliveries: NotifyDeliveriesResponse;
  targets: NotifyTargetView[];
  filters: DeliveryFilters;
  onFilters(filters: DeliveryFilters): void;
  text: LocalizedText;
}): JSX.Element {
  const sms = deliveries.sms_usage;
  return <div className="history-page notify-page">
    <p className="history-muted">{text(`Kept ${deliveries.retention_days} days · only results and error categories, never message text or secrets`, `保留 ${deliveries.retention_days} 天 · 只记结果与错误类别，不记消息正文与密钥`)}</p>
    <div className="notify-kpis">
      <div className="history-card"><span className="history-muted">{text("Sent · last 24 hours", "最近 24 小时 · 已发送")}</span><strong>{deliveries.sent_24h}</strong></div>
      <div className="history-card"><span className="history-muted">{text("Failed (retries used up)", "失败（已用完重试）")}</span><strong className={deliveries.failed_24h > 0 ? "bad" : ""}>{deliveries.failed_24h}</strong></div>
      <div className="history-card"><span className="history-muted">{text("Waiting to retry", "等待重试")}</span><strong>{deliveries.pending}</strong></div>
      <div className="history-card"><span className="history-muted">{text("SMS this hour / limit", "短信本小时用量 / 上限")}</span><strong>{sms.length === 0 ? "—" : sms.map((usage) => `${usage.used} / ${usage.hourly_limit}`).join(" · ")}</strong></div>
    </div>
    <section aria-labelledby="notify-log" className="history-card">
      <div className="history-card-head">
        <h2 id="notify-log">{text("Delivery log", "投递记录")}</h2>
        <label className="notify-filter">{text("Channel", "渠道")}
          <select onChange={(event) => onFilters({ ...filters, kind: (event.target.value || null) as NotifyChannelKind | null })} value={filters.kind ?? ""}>
            <option value="">{text("All", "全部")}</option>
            {NOTIFY_KINDS.map((kind) => <option key={kind} value={kind}>{shortKindName(kind, text)}</option>)}
          </select>
        </label>
        <label className="notify-filter">{text("Target", "发送对象")}
          <select onChange={(event) => onFilters({ ...filters, target_id: event.target.value || null })} value={filters.target_id ?? ""}>
            <option value="">{text("All", "全部")}</option>
            {targets.map((target) => <option key={target.target_id} value={target.target_id}>{target.name}</option>)}
          </select>
        </label>
        <label className="notify-filter">{text("Result", "结果")}
          <select onChange={(event) => onFilters({ ...filters, state: (event.target.value || null) as NotifyDeliveryState | null })} value={filters.state ?? ""}>
            <option value="">{text("All", "全部")}</option>
            {(["sent", "failed", "pending", "skipped"] as NotifyDeliveryState[]).map((state) => <option key={state} value={state}>{deliveryStateText(state, text)}</option>)}
          </select>
        </label>
      </div>
      {deliveries.deliveries.length === 0 ? <p className="notify-empty">{text("Nothing sent yet.", "还没有投递记录。")}</p> : <div className="history-table-wrap">
        <table className="history-table notify-table">
          <thead><tr>
            <th>{text("Time (UTC)", "时间（UTC）")}</th><th>{text("Channel · target", "渠道 · 发送对象")}</th><th>{text("Content", "内容")}</th>
            <th>{text("Result", "结果")}</th><th className="num">{text("Attempts", "尝试")}</th><th>{text("Notes", "说明")}</th>
          </tr></thead>
          <tbody>
            {deliveries.deliveries.map((row) => {
              const notes = [
                row.error ? `${errorText(row.error, text)}${row.provider_code ? ` · ${row.provider_code}` : ""}` : null,
                row.next_attempt_seconds !== null ? text(`retry at ${clockText(row.next_attempt_seconds).slice(11, 16)}`, `${clockText(row.next_attempt_seconds).slice(11, 16)} 重试`) : null,
                row.duration_ms !== null && row.state !== "skipped" ? durationText(row.duration_ms) : null,
              ].filter((note): note is string => note !== null);
              return <tr key={row.delivery_id}>
                <td className="mono">{clockText(row.at_seconds).slice(5, 19)}</td>
                <td>{shortKindName(row.kind, text)} · {row.target_name}</td>
                <td>{eventText(row.event, row.alert_count, row.cluster, text)}</td>
                <td><span className={`notify-chip ${row.state === "sent" ? "ok" : row.state === "failed" ? "bad" : row.state === "pending" ? "warn" : "off"}`}>{deliveryStateText(row.state, text)}</span></td>
                <td className="num">{row.attempts}</td>
                <td className="muted">{notes.join(" · ") || "—"}</td>
              </tr>;
            })}
          </tbody>
        </table>
      </div>}
      <ul className="notify-notes">
        <li>{text("Each send times out after 10 seconds; network failures retry after 1, 5 and 30 minutes (4 attempts), then count as failed.", "每次发送超时 10 秒；网络类失败按 1 分钟、5 分钟、30 分钟重试，共 4 次，之后记为失败，不再重试。")}</li>
        <li>{text("SMS above the channel's hourly limit is skipped and marked; the alerts stay in the app.", "短信超出渠道每小时上限时跳过并标记，告警仍保留在应用内。")}</li>
        <li>{text("Notes keep only a category and the provider's short code, never its reply text.", "说明只记错误类别与平台返回码，不记录返回正文。")}</li>
      </ul>
    </section>
  </div>;
}
