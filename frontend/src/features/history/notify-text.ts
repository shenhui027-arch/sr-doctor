// Words for the notification views. The server decides every state and
// error; these only name them.
import type {
  NotifyChannelConfig, NotifyChannelKind, NotifyChannelView, NotifyDeliveryState, NotifyErrorClass, NotifyEvent,
  NotifySeverity, NotifyTargetInput, NotifyTargetView, NotifyTestResponse,
} from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";

export const NOTIFY_KINDS: NotifyChannelKind[] = ["email", "feishu", "dingtalk", "aliyun_sms", "tencent_sms"];

export function kindName(kind: NotifyChannelKind, text: LocalizedText): string {
  return {
    email: text("Email · SMTP", "邮件 · SMTP"),
    feishu: text("Feishu · custom bot", "飞书 · 自定义机器人"),
    dingtalk: text("DingTalk · custom bot", "钉钉 · 自定义机器人"),
    aliyun_sms: text("SMS · Alibaba Cloud", "短信 · 阿里云"),
    tencent_sms: text("SMS · Tencent Cloud", "短信 · 腾讯云"),
  }[kind];
}

export function shortKindName(kind: NotifyChannelKind, text: LocalizedText): string {
  return { email: text("Email", "邮件"), feishu: text("Feishu", "飞书"), dingtalk: text("DingTalk", "钉钉"), aliyun_sms: text("SMS · Alibaba", "短信 · 阿里云"), tencent_sms: text("SMS · Tencent", "短信 · 腾讯云") }[kind];
}

/** What one target of the channel is called. */
export function targetNoun(kind: NotifyChannelKind, text: LocalizedText): string {
  if (kind === "email") return text("recipient", "收件人");
  if (kind === "feishu" || kind === "dingtalk") return text("group bot", "群机器人");
  return text("phone number", "手机号");
}

export function severityText(value: NotifySeverity, text: LocalizedText): string {
  return value === "high" ? text("High only", "仅高") : text("Medium and up", "中及以上");
}

export function errorText(error: NotifyErrorClass, text: LocalizedText): string {
  return {
    network: text("Network unreachable", "网络不可达"),
    timeout: text("Timed out", "超时"),
    tls: text("TLS verification failed", "TLS 校验失败"),
    auth_failed: text("Authentication failed", "认证失败"),
    rejected: text("Refused by the provider", "对方拒绝"),
    invalid_response: text("Unreadable reply", "返回无法识别"),
    rate_limited: text("Over the hourly limit", "超出频率上限"),
    not_configured: text("Channel or target not set up or switched off", "渠道或对象未设置或已停用"),
    secret_unreadable: text("Saved secret must be entered again", "已保存的密钥需重新填写"),
  }[error];
}

export function deliveryStateText(state: NotifyDeliveryState, text: LocalizedText): string {
  return { pending: text("Waiting to retry", "等待重试"), sent: text("Sent", "成功"), failed: text("Failed", "失败"), skipped: text("Skipped", "已跳过") }[state];
}

export function eventText(event: NotifyEvent, count: number, cluster: string | null, text: LocalizedText): string {
  if (event === "test") return text("Test message", "测试消息");
  const where = cluster ? ` · ${cluster}` : "";
  return event === "resolved"
    ? text(`${count} resolved${where}`, `${count} 条已解决${where}`)
    : text(`${count} new alert${count === 1 ? "" : "s"}${where}`, `${count} 条新告警${where}`);
}

/** Under a second in ms, otherwise seconds with one decimal. */
export function durationText(ms: number): string {
  return ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(1)} s`;
}

export function testText(result: NotifyTestResponse, text: LocalizedText): string {
  if (result.state === "sent") return text(`Test sent · ${durationText(result.duration_ms)}`, `测试已发送 · ${durationText(result.duration_ms)}`);
  const why = result.error ? errorText(result.error, text) : "";
  const code = result.provider_code ? ` ${result.provider_code}` : "";
  return result.state === "skipped" ? text(`Test skipped · ${why}`, `测试已跳过 · ${why}`) : text(`Test failed · ${why}${code}`, `测试失败 · ${why}${code}`);
}

/** Refused requests, in words; unknown codes keep the code. */
export function notifyActionText(code: string, text: LocalizedText): string {
  return {
    invalid_notify_target: text("Check the target: name, address or phone number.", "请检查发送对象：名称、地址或手机号。"),
    invalid_notify_webhook: text("Only official Feishu (open.feishu.cn, open.larksuite.com) and DingTalk (oapi.dingtalk.com) webhooks are accepted.", "Webhook 只接受飞书（open.feishu.cn、open.larksuite.com）和钉钉（oapi.dingtalk.com）的官方地址。"),
    invalid_notify_channel: text("Check the channel settings.", "请检查渠道设置。"),
    invalid_notify_settings: text("The address must start with http:// or https://.", "访问地址需以 http:// 或 https:// 开头。"),
    notify_target_limit: text("A channel takes at most 20 targets.", "每个渠道最多 20 个发送对象。"),
    notify_target_not_found: text("That target no longer exists.", "这个发送对象已不存在。"),
    notify_secret_unreadable: text("The saved secret cannot be decrypted (the master key changed); enter it again.", "已保存的密钥无法解密（主密钥已更换），请重新填写。"),
    notify_key_unavailable: text("Notifications are off: the master key is not available. See the server log.", "通知不可用：主密钥未就绪，请查看服务日志。"),
    history_disabled: text("Notifications need the local history store, which is off.", "通知需要本地历史库，当前历史库已关闭。"),
  }[code] ?? text(`Request failed (${code})`, `请求失败（${code}）`);
}

export type Tone = "ok" | "bad" | "off" | "warn";

/** One status chip per channel card, most urgent first. */
export function channelStatus(channel: NotifyChannelView, targets: NotifyTargetView[], text: LocalizedText): { label: string; tone: Tone } {
  const mine = targets.filter((target) => target.kind === channel.kind);
  if (channel.secret_unreadable || mine.some((target) => target.secret_unreadable)) return { label: text("Re-enter secrets", "需重新填写"), tone: "warn" };
  const needsSettings = channel.kind === "email" || channel.kind === "aliyun_sms" || channel.kind === "tencent_sms";
  if ((needsSettings && channel.config === null) || channel.targets === 0) return { label: text("Not set up", "未配置"), tone: "off" };
  if (channel.failing_targets > 0) return { label: text(`${channel.failing_targets} failing`, `${channel.failing_targets} 个失败`), tone: "bad" };
  return channel.enabled ? { label: text("On", "已启用"), tone: "ok" } : { label: text("Off", "未启用"), tone: "off" };
}

export function defaultConfig(kind: NotifyChannelKind): NotifyChannelConfig {
  switch (kind) {
    case "email": return { kind, host: "", port: 465, security: "tls", username: null, from_address: "", from_name: "SR Doctor" };
    case "feishu": return { kind };
    case "dingtalk": return { kind };
    case "aliyun_sms": return { kind, access_key_id: "", sign_name: "", template_code: "", region: "cn_hangzhou", hourly_limit: 10 };
    case "tencent_sms": return { kind, secret_id: "", sdk_app_id: "", sign_name: "", template_id: "", region: "ap_guangzhou", hourly_limit: 10 };
  }
}

export function emptyTarget(kind: NotifyChannelKind): NotifyTargetInput {
  return { target_id: null, kind, name: "", enabled: true, min_severity: "medium", address: null, webhook: null, signing_secret: null, clear_signing_secret: false };
}

/** A saved target as an input that keeps every saved secret. */
export function targetInput(view: NotifyTargetView, changes: Partial<NotifyTargetInput> = {}): NotifyTargetInput {
  return {
    target_id: view.target_id, kind: view.kind, name: view.name, enabled: view.enabled, min_severity: view.min_severity,
    address: view.kind === "email" ? view.address_hint : null, webhook: null, signing_secret: null, clear_signing_secret: false,
    ...changes,
  };
}
