import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { NotifyDeliveriesResponse, NotifyOverviewResponse } from "../../../../contracts/generated/doctor-contracts";
import { NotifyDialog } from "./NotifyDialog";
import { DeliveriesPanel, NotifyPanel } from "./NotifyPanel";
import { channelStatus, notifyActionText, targetInput } from "./notify-text";

const english = (value: string): string => value;
const chinese = (_english: string, value: string): string => value;
const noop = () => undefined;
const yes = () => Promise.resolve(true);

const overview: NotifyOverviewResponse = {
  settings: { notify_resolved: false, include_template: false, language: "zh", public_url: "http://10.0.0.5:8080" },
  channels: [
    { kind: "email", enabled: true, config: { kind: "email", host: "smtp.example.com", port: 465, security: "tls", username: "alerts@example.com", from_address: "alerts@example.com", from_name: "SR Doctor" }, secret_saved: true, secret_unreadable: false, targets: 2, enabled_targets: 1, failing_targets: 0 },
    { kind: "feishu", enabled: true, config: { kind: "feishu" }, secret_saved: false, secret_unreadable: false, targets: 1, enabled_targets: 1, failing_targets: 1 },
    { kind: "dingtalk", enabled: false, config: { kind: "dingtalk" }, secret_saved: false, secret_unreadable: false, targets: 0, enabled_targets: 0, failing_targets: 0 },
    { kind: "aliyun_sms", enabled: false, config: null, secret_saved: false, secret_unreadable: false, targets: 0, enabled_targets: 0, failing_targets: 0 },
    { kind: "tencent_sms", enabled: false, config: null, secret_saved: false, secret_unreadable: false, targets: 0, enabled_targets: 0, failing_targets: 0 },
  ],
  targets: [
    { target_id: "1", kind: "email", name: "DBA 值班", enabled: true, min_severity: "medium", address_hint: "dba@example.com", signing_secret_saved: false, secret_unreadable: false, last: { at_seconds: 1_793_264_400, state: "sent", error: null, provider_code: null } },
    { target_id: "2", kind: "email", name: "外包运维", enabled: false, min_severity: "high", address_hint: "ops@vendor.example", signing_secret_saved: false, secret_unreadable: false, last: null },
    { target_id: "3", kind: "feishu", name: "DBA 值班群", enabled: true, min_severity: "medium", address_hint: "open.feishu.cn/…/****9f2c", signing_secret_saved: true, secret_unreadable: false, last: { at_seconds: 1_793_264_400, state: "failed", error: "auth_failed", provider_code: "19021" } },
  ],
  remote_collection_allowed: false,
  key_source: "file",
  max_targets_per_channel: 20,
  limitations: [],
};

describe("notification channels (design board N1)", () => {
  it("shows every channel as a card, every target in one list and the G1 notice", () => {
    const html = renderToStaticMarkup(<NotifyPanel busy={false} onOpen={noop} onSaveSettings={noop} onTest={noop} overview={overview} tests={{}} text={chinese} />);
    expect(html).toContain("远端集群在 G1 生产只读试点通过前不做后台采集");
    expect(html.match(/aria-haspopup="dialog" class="notify-card"/g)).toHaveLength(5);
    expect(html).toContain("2 个发送对象 · 启用 1");
    expect(html).toContain("1 个失败");
    expect(html).toContain("共 3 个 · 启用 2 个");
    expect(html).toContain("open.feishu.cn/…/****9f2c");
    expect(html).toContain("失败 · 10-29 09:00 · 认证失败 19021");
    expect(html).toContain("仅高");
    expect(html).toContain("value=\"http://10.0.0.5:8080\"");
  });

  it("names the most urgent channel state first", () => {
    const [email, feishu, dingtalk, aliyun] = overview.channels;
    expect(channelStatus(email!, overview.targets, english)).toEqual({ label: "On", tone: "ok" });
    expect(channelStatus(feishu!, overview.targets, english)).toEqual({ label: "1 failing", tone: "bad" });
    expect(channelStatus(dingtalk!, overview.targets, english).label).toBe("Not set up");
    expect(channelStatus(aliyun!, overview.targets, english).label).toBe("Not set up");
    expect(channelStatus({ ...email!, secret_unreadable: true }, overview.targets, english).label).toBe("Re-enter secrets");
  });

  it("explains refusals instead of showing codes", () => {
    expect(notifyActionText("invalid_notify_webhook", chinese)).toContain("open.feishu.cn");
    expect(notifyActionText("notify_target_limit", english)).toBe("A channel takes at most 20 targets.");
    expect(notifyActionText("other", english)).toBe("Request failed (other)");
  });

  it("re-sends a saved target without its secrets, so the server keeps them", () => {
    const feishu = overview.targets[2]!;
    expect(targetInput(feishu, { enabled: false })).toEqual({ target_id: "3", kind: "feishu", name: "DBA 值班群", enabled: false, min_severity: "medium", address: null, webhook: null, signing_secret: null, clear_signing_secret: false });
    expect(targetInput(overview.targets[0]!).address).toBe("dba@example.com");
  });
});

describe("channel dialog (design board N1 dialog)", () => {
  const dialog = (kind: "email" | "feishu" | "dingtalk", edit: string | "new" | null) => renderToStaticMarkup(
    <NotifyDialog busy={false} error={null} focus={{ kind, edit }} onClose={noop} onDeleteTarget={noop} onSaveChannel={yes} onSaveTarget={yes} onTest={noop} overview={overview} tests={{}} text={chinese} />,
  );

  it("puts channel settings above the channel's targets and never fills a secret field", () => {
    const html = dialog("email", null);
    expect(html).toContain("role=\"dialog\"");
    expect(html).toContain("aria-modal=\"true\"");
    expect(html.indexOf("渠道设置")).toBeLessThan(html.indexOf("发送对象"));
    expect(html).toContain("口令 / 授权码 · 只写");
    expect(html).toContain("placeholder=\"已保存 · 留空表示不修改\" type=\"password\" value=\"\"");
    expect(html).toContain("dba@example.com");
    expect(html).toContain("清除渠道密钥");
    expect(html).not.toContain("notify-target-form");
  });

  it("edits a bot target in place with write-only webhook and signing secret", () => {
    const html = dialog("feishu", "3");
    expect(html).toContain("编辑群机器人：DBA 值班群");
    expect(html).toContain("已保存：open.feishu.cn/…/****9f2c");
    expect(html).toContain("删除已保存的签名密钥");
    expect(html).toContain("先发一条测试");
    expect(html).not.toContain("清除渠道密钥");
  });

  it("opens the add form when the channel has no target yet", () => {
    const html = dialog("dingtalk", null);
    expect(html).toContain("还没有群机器人。添加后这个渠道才会发出消息。");
    expect(html).toContain("添加群机器人</strong>");
    expect(html).toContain("oapi.dingtalk.com/robot/send?access_token=");
  });
});

describe("delivery log (design board N2)", () => {
  const deliveries: NotifyDeliveriesResponse = {
    deliveries: [
      { delivery_id: "9", at_seconds: 1_793_264_400, kind: "dingtalk", target_id: "4", target_name: "数据平台群", event: "new_alerts", cluster: "10.0.0.5:9030", alert_count: 2, state: "pending", attempts: 1, next_attempt_seconds: 1_793_264_460, error: "timeout", provider_code: null, duration_ms: 10_000 },
      { delivery_id: "8", at_seconds: 1_793_260_800, kind: "aliyun_sms", target_id: "5", target_name: "李工", event: "new_alerts", cluster: "10.0.0.5:9030", alert_count: 1, state: "skipped", attempts: 0, next_attempt_seconds: null, error: "rate_limited", provider_code: null, duration_ms: null },
      { delivery_id: "7", at_seconds: 1_793_250_000, kind: "email", target_id: null, target_name: "已删除", event: "test", cluster: null, alert_count: 0, state: "sent", attempts: 1, next_attempt_seconds: null, error: null, provider_code: null, duration_ms: 1_300 },
    ],
    sent_24h: 14, failed_24h: 2, pending: 1, sms_usage: [{ kind: "aliyun_sms", used: 3, hourly_limit: 10 }], retention_days: 30,
  };

  it("says what was sent to whom, what happened and when it retries", () => {
    const html = renderToStaticMarkup(<DeliveriesPanel deliveries={deliveries} filters={{ kind: null, target_id: null, state: null }} onFilters={noop} targets={overview.targets} text={chinese} />);
    expect(html).toContain("保留 30 天");
    expect(html).toContain("3 / 10");
    expect(html).toContain("钉钉 · 数据平台群");
    expect(html).toContain("2 条新告警 · 10.0.0.5:9030");
    expect(html).toContain("超时 · 09:01 重试 · 10.0 s");
    expect(html).toContain("1.3 s");
    expect(html).toContain("超出频率上限");
    expect(html).toContain("测试消息");
    expect(html).toContain("<option value=\"3\">DBA 值班群</option>");
  });
});
