// Design board 08-settings dialog: one channel's settings (how to send) above
// its targets (to whom). Secret fields are write-only: empty means "keep the
// saved one"; the server never sends a secret back. Uses the Dialog primitive.
import { useState } from "react";
import type { JSX, ReactNode } from "react";
import type {
  AliyunSmsRegion,
  EmailSecurity,
  NotifyChannelConfig,
  NotifyChannelKind,
  NotifyChannelSaveRequest,
  NotifyOverviewResponse,
  NotifySeverity,
  NotifyTargetInput,
  NotifyTargetView,
  TencentSmsRegion,
} from "../../../../contracts/generated/doctor-contracts";
import { Button, Dialog } from "../../design";
import { lookupEnumLabel } from "../../i18n/enum-label";
import type { LocalizedText } from "../../i18n/locale";
import { TestChip, type TestResults } from "./NotifyPanel";
import {
  defaultConfig,
  deliveryStateText,
  emptyTarget,
  errorText,
  kindName,
  notifyActionText,
  severityText,
  targetInput,
  targetNoun,
} from "./notify-text";

/** `new` opens an empty target form; a target id opens that target. */
export type DialogFocus = { kind: NotifyChannelKind; edit: string | "new" | null };

function Field({ label, wide, hint, children }: { label: string; wide?: boolean; hint?: string; children: ReactNode }): JSX.Element {
  return (
    <label className={wide ? "field form-span-2" : "field"}>
      <span className="field-label">{label}</span>
      {children}
      {hint ? <span className="card-subtitle">{hint}</span> : null}
    </label>
  );
}

function secretPlaceholder(saved: boolean, text: LocalizedText): string {
  return saved ? text("Saved · leave empty to keep", "已保存 · 留空表示不修改") : text("Not saved", "未保存");
}

const SUBTITLE_LABELS: Record<NotifyChannelKind, readonly [string, string]> = {
  email: [
    "One SMTP server, many recipients. Prefer the mailbox's app password over its login password.",
    "一个 SMTP 服务器，发往多个收件人；建议使用邮箱的授权码而不是登录密码",
  ],
  feishu: [
    "Each target is a custom bot in one group: group settings › bots › add a custom bot",
    "每个发送对象是一个群里的自定义机器人：群设置 › 群机器人 › 添加自定义机器人",
  ],
  dingtalk: [
    "Each target is a custom bot in one group: group settings › bots › add › custom",
    "每个发送对象是一个群里的自定义机器人：群设置 › 机器人 › 添加机器人 › 自定义",
  ],
  aliyun_sms: [
    "One AccessKey with an approved signature and template, many phone numbers",
    "一套 AccessKey 与已审核的签名、模板，发往多个手机号",
  ],
  tencent_sms: [
    "One SecretId/SecretKey with an approved signature and template, many phone numbers",
    "一套 SecretId/SecretKey 与已审核的签名、模板，发往多个手机号",
  ],
};

function subtitle(kind: NotifyChannelKind, text: LocalizedText): string {
  return lookupEnumLabel(SUBTITLE_LABELS, kind, text);
}

function notes(kind: NotifyChannelKind, text: LocalizedText): string[] {
  switch (kind) {
    case "email":
      return [
        text("A recipient can be a personal mailbox or a mailing list.", "收件人可以是个人邮箱，也可以是邮件组地址。"),
        text(
          "TLS modes always verify the server certificate; \"no encryption\" sends the password and message in clear text.",
          "加密方式始终校验服务器证书；选“不加密”时口令与告警内容明文经过网络。",
        ),
      ];
    case "feishu":
      return [
        text(
          "Feishu has no channel settings; each group bot has its own webhook and signing secret. Only open.feishu.cn and open.larksuite.com are accepted, with full HTTPS certificate verification.",
          "飞书没有渠道级设置：每个群机器人各有自己的 Webhook 与签名密钥。地址只接受官方主机 open.feishu.cn 与 open.larksuite.com，且强制完整校验 HTTPS 证书。",
        ),
        text("If the bot uses custom keywords, include SR Doctor.", "机器人若设了“自定义关键词”，请包含 SR Doctor。"),
      ];
    case "dingtalk":
      return [
        text(
          "DingTalk has no channel settings; each group bot has its own webhook and signing secret. Only oapi.dingtalk.com is accepted, with full HTTPS certificate verification.",
          "钉钉没有渠道级设置：每个群机器人各有自己的 Webhook 与加签密钥。地址只接受官方主机 oapi.dingtalk.com，且强制完整校验 HTTPS 证书。",
        ),
        text("Choose \"sign\" as the security setting; with custom keywords, include SR Doctor.", "安全设置选“加签”；若用“自定义关键词”，请包含 SR Doctor。"),
      ];
    case "aliyun_sms":
      return [
        text(
          "Template example: SR Doctor: cluster ${cluster} has ${count} new regression alerts, highest severity ${level}.",
          "模板正文示例：SR Doctor：集群${cluster}新增${count}条性能回归告警，最高严重度${level}，请登录查看。",
        ),
        text("Use a RAM user allowed to send SMS only.", "建议 RAM 子账号，只授予短信发送权限。"),
      ];
    case "tencent_sms":
      return [
        text("The template takes variables in order: {1} cluster, {2} count, {3} severity.", "模板按顺序取变量：{1} 集群、{2} 条数、{3} 严重度。"),
        text("Use a sub-user allowed to send SMS only.", "建议子用户，只授予短信发送权限。"),
      ];
    default:
      return [];
  }
}

function ChannelFields({
  config,
  onChange,
  secret,
  onSecret,
  secretSaved,
  text,
}: {
  config: NotifyChannelConfig;
  onChange(config: NotifyChannelConfig): void;
  secret: string;
  onSecret(value: string): void;
  secretSaved: boolean;
  text: LocalizedText;
}): JSX.Element | null {
  const secretField = (label: string) => (
    <Field label={`${label} · ${text("write-only", "只写")}`}>
      <input
        autoComplete="new-password"
        className="input"
        onChange={(event) => onSecret(event.target.value)}
        placeholder={secretPlaceholder(secretSaved, text)}
        type="password"
        value={secret}
      />
    </Field>
  );

  switch (config.kind) {
    case "email":
      return (
        <div className="form-grid-2">
          <Field label={text("SMTP host", "SMTP 主机")}>
            <input className="input mono" onChange={(event) => onChange({ ...config, host: event.target.value.trim() })} placeholder="smtp.example.com" value={config.host} />
          </Field>
          <Field label={text("Port", "端口")}>
            <input className="input mono" inputMode="numeric" onChange={(event) => onChange({ ...config, port: Number(event.target.value.replace(/\D/g, "")) || 0 })} value={config.port || ""} />
          </Field>
          <Field label={text("Encryption", "加密方式")}>
            <select className="select" onChange={(event) => onChange({ ...config, security: event.target.value as EmailSecurity })} value={config.security}>
              <option value="tls">{text("SSL/TLS (465)", "SSL/TLS（465）")}</option>
              <option value="starttls">{text("STARTTLS (587)", "STARTTLS（587）")}</option>
              <option value="plain">{text("None (password and message in clear text)", "不加密（口令与内容明文传输）")}</option>
            </select>
          </Field>
          <Field label={text("Sender address", "发件人地址")}>
            <input className="input mono" onChange={(event) => onChange({ ...config, from_address: event.target.value.trim() })} placeholder="alerts@example.com" value={config.from_address} />
          </Field>
          <Field label={text("Sender name", "发件人名称")}>
            <input className="input" onChange={(event) => onChange({ ...config, from_name: event.target.value || null })} value={config.from_name ?? ""} />
          </Field>
          <Field label={text("Username (empty: no login)", "用户名（留空表示不登录）")}>
            <input autoComplete="off" className="input mono" onChange={(event) => onChange({ ...config, username: event.target.value.trim() || null })} value={config.username ?? ""} />
          </Field>
          {secretField(text("Password / app password", "口令 / 授权码"))}
          {config.security === "plain" ? (
            <div className="notice-banner notice-banner--warn form-span-2" role="note">
              <span>{text("Without encryption the password and alert content cross the network in clear text.", "不加密时，口令与告警内容将明文经过网络。")}</span>
            </div>
          ) : null}
        </div>
      );
    case "aliyun_sms":
      return (
        <div className="form-grid-2">
          <Field label="AccessKey ID">
            <input className="input mono" onChange={(event) => onChange({ ...config, access_key_id: event.target.value.trim() })} placeholder="LTAI…" value={config.access_key_id} />
          </Field>
          {secretField("AccessKey Secret")}
          <Field label={text("Signature name", "签名名称")}>
            <input className="input" onChange={(event) => onChange({ ...config, sign_name: event.target.value })} value={config.sign_name} />
          </Field>
          <Field label={text("Template code", "模板 CODE")}>
            <input className="input mono" onChange={(event) => onChange({ ...config, template_code: event.target.value.trim() })} placeholder="SMS_123456789" value={config.template_code} />
          </Field>
          <Field label={text("Region", "地域")}>
            <select className="select" onChange={(event) => onChange({ ...config, region: event.target.value as AliyunSmsRegion })} value={config.region}>
              <option value="cn_hangzhou">{text("cn-hangzhou (default)", "cn-hangzhou（默认）")}</option>
              <option value="ap_southeast1">ap-southeast-1</option>
            </select>
          </Field>
          <Field label={text("At most per hour (whole channel)", "每小时最多条数（整个渠道）")}>
            <input className="input mono" inputMode="numeric" onChange={(event) => onChange({ ...config, hourly_limit: Number(event.target.value.replace(/\D/g, "")) || 0 })} value={config.hourly_limit || ""} />
          </Field>
        </div>
      );
    case "tencent_sms":
      return (
        <div className="form-grid-2">
          <Field label="SecretId">
            <input className="input mono" onChange={(event) => onChange({ ...config, secret_id: event.target.value.trim() })} placeholder="AKID…" value={config.secret_id} />
          </Field>
          {secretField("SecretKey")}
          <Field label="SdkAppId">
            <input className="input mono" inputMode="numeric" onChange={(event) => onChange({ ...config, sdk_app_id: event.target.value.trim() })} placeholder="1400…" value={config.sdk_app_id} />
          </Field>
          <Field label={text("Signature", "签名内容")}>
            <input className="input" onChange={(event) => onChange({ ...config, sign_name: event.target.value })} value={config.sign_name} />
          </Field>
          <Field label={text("Template ID", "模板 ID")}>
            <input className="input mono" inputMode="numeric" onChange={(event) => onChange({ ...config, template_id: event.target.value.trim() })} value={config.template_id} />
          </Field>
          <Field label={text("Region", "地域")}>
            <select className="select" onChange={(event) => onChange({ ...config, region: event.target.value as TencentSmsRegion })} value={config.region}>
              <option value="ap_guangzhou">{text("ap-guangzhou (default)", "ap-guangzhou（默认）")}</option>
              <option value="ap_beijing">ap-beijing</option>
              <option value="ap_nanjing">ap-nanjing</option>
            </select>
          </Field>
          <Field label={text("At most per hour (whole channel)", "每小时最多条数（整个渠道）")}>
            <input className="input mono" inputMode="numeric" onChange={(event) => onChange({ ...config, hourly_limit: Number(event.target.value.replace(/\D/g, "")) || 0 })} value={config.hourly_limit || ""} />
          </Field>
        </div>
      );
    default:
      return null;
  }
}

function TargetForm({
  kind,
  saved,
  onSubmit,
  onTest,
  onCancel,
  busy,
  testResult,
  text,
}: {
  kind: NotifyChannelKind;
  saved: NotifyTargetView | null;
  onSubmit(input: NotifyTargetInput): void;
  onTest(input: NotifyTargetInput): void;
  onCancel(): void;
  busy: boolean;
  testResult: TestResults[string] | undefined;
  text: LocalizedText;
}): JSX.Element {
  const [input, setInput] = useState<NotifyTargetInput>(() => (saved ? targetInput(saved) : emptyTarget(kind)));
  const isBot = kind === "feishu" || kind === "dingtalk";
  const isSms = kind === "aliyun_sms" || kind === "tencent_sms";
  const noun = targetNoun(kind, text);
  const set = (changes: Partial<NotifyTargetInput>) => setInput((current) => ({ ...current, ...changes }));

  return (
    <form className="notify-target-form" onSubmit={(event) => { event.preventDefault(); onSubmit(input); }}>
      <div className="notify-target-form-hd">
        <strong>{saved ? text(`Edit ${noun}: ${saved.name}`, `编辑${noun}：${saved.name}`) : text(`Add a ${noun}`, `添加${noun}`)}</strong>
        <span className="badge-stamp">{text("AES-256-GCM write-only", "AES-256-GCM 只写不读")}</span>
      </div>
      <div className="form-grid-2">
        <Field label={text("Name", "名称")}>
          <input
            className="input"
            maxLength={40}
            onChange={(event) => set({ name: event.target.value })}
            placeholder={isBot ? text("e.g. DBA on-call group", "例如 DBA 值班群") : text("e.g. DBA on-call", "例如 DBA 值班")}
            required
            value={input.name}
          />
        </Field>
        <Field label={text("Receives", "接收严重度")}>
          <select className="select" onChange={(event) => set({ min_severity: event.target.value as NotifySeverity })} value={input.min_severity}>
            <option value="medium">{severityText("medium", text)}</option>
            <option value="high">{severityText("high", text)}</option>
          </select>
        </Field>
        {kind === "email" ? (
          <Field label={text("Email address", "邮箱地址")} wide>
            <input className="input mono" onChange={(event) => set({ address: event.target.value.trim() || null })} placeholder="name@example.com" required type="email" value={input.address ?? ""} />
          </Field>
        ) : null}
        {isSms ? (
          <Field hint={saved ? text(`Saved: ${saved.address_hint}. Leave empty to keep.`, `已保存：${saved.address_hint}，留空表示不修改`) : undefined} label={text("Phone number", "手机号")} wide>
            <input className="input mono" inputMode="tel" onChange={(event) => set({ address: event.target.value.trim() || null })} placeholder={text("11-digit mobile, or +country code", "11 位手机号，或 +国家区号")} required={!saved} value={input.address ?? ""} />
          </Field>
        ) : null}
        {isBot ? (
          <>
            <Field
              hint={saved ? text(`Saved: ${saved.address_hint}`, `已保存：${saved.address_hint}`) : undefined}
              label={`${kind === "feishu" ? text("Webhook address", "Webhook 地址") : text("Webhook address (with access_token)", "Webhook 地址（含 access_token）")} · ${text("write-only", "只写")}`}
              wide
            >
              <input
                autoComplete="off"
                className="input mono"
                onChange={(event) => set({ webhook: event.target.value.trim() || null })}
                placeholder={saved ? secretPlaceholder(true, text) : kind === "feishu" ? "https://open.feishu.cn/open-apis/bot/v2/hook/…" : "https://oapi.dingtalk.com/robot/send?access_token=…"}
                required={!saved}
                type="password"
                value={input.webhook ?? ""}
              />
            </Field>
            <Field
              label={`${kind === "feishu" ? text("Signing secret (when the bot checks signatures)", "签名密钥（机器人开启“签名校验”时填写）") : text("Signing secret (starts with SEC)", "加签密钥（以 SEC 开头）")} · ${text("write-only", "只写")}`}
              wide
            >
              <input
                autoComplete="new-password"
                className="input mono"
                disabled={input.clear_signing_secret}
                onChange={(event) => set({ signing_secret: event.target.value.trim() || null })}
                placeholder={secretPlaceholder(Boolean(saved?.signing_secret_saved), text)}
                type="password"
                value={input.signing_secret ?? ""}
              />
            </Field>
            {saved?.signing_secret_saved ? (
              <label className="check-inline form-span-2">
                <input
                  checked={input.clear_signing_secret}
                  onChange={(event) => set({ clear_signing_secret: event.target.checked, signing_secret: null })}
                  type="checkbox"
                />
                <span>{text("Remove the saved signing secret", "删除已保存的签名密钥")}</span>
              </label>
            ) : null}
          </>
        ) : null}
      </div>
      <div className="notify-actions">
        <Button disabled={busy} size="sm" type="submit" variant="primary">{saved ? text("Update", "更新") : text("Add", "添加")}</Button>
        <Button disabled={busy || testResult === "running"} onClick={() => onTest(input)} size="sm" variant="secondary">
          {text("Send a test first", "先发一条测试")}
        </Button>
        <Button onClick={onCancel} size="sm" variant="ghost">{text("Cancel", "取消")}</Button>
        <TestChip result={testResult} text={text} />
      </div>
    </form>
  );
}

export function NotifyDialog({
  focus,
  overview,
  busy,
  error,
  tests,
  onClose,
  onSaveChannel,
  onSaveTarget,
  onDeleteTarget,
  onTest,
  text,
}: {
  focus: DialogFocus;
  overview: NotifyOverviewResponse;
  busy: boolean;
  error: string | null;
  tests: TestResults;
  onClose(): void;
  onSaveChannel(request: NotifyChannelSaveRequest): Promise<boolean>;
  onSaveTarget(input: NotifyTargetInput): Promise<boolean>;
  onDeleteTarget(targetId: string): void;
  onTest(input: NotifyTargetInput, key: string): void;
  text: LocalizedText;
}): JSX.Element | null {
  const channel = overview.channels.find((item) => item.kind === focus.kind);
  const targets = overview.targets.filter((target) => target.kind === focus.kind);
  const [config, setConfig] = useState<NotifyChannelConfig>(() => channel?.config ?? defaultConfig(focus.kind));
  const [enabled, setEnabled] = useState(() => channel?.enabled ?? false);
  const [secret, setSecret] = useState("");
  const [edit, setEdit] = useState<string | "new" | null>(() => focus.edit ?? (targets.length === 0 ? "new" : null));
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  if (!channel) return null;
  const kind = focus.kind;
  const noun = targetNoun(kind, text);
  const hasSecret = kind === "email" || kind === "aliyun_sms" || kind === "tencent_sms";
  const editing = edit && edit !== "new" ? targets.find((target) => target.target_id === edit) ?? null : null;

  const save = () => {
    const request: NotifyChannelSaveRequest = { config, enabled, secret: secret || null, clear_secret: false };
    void onSaveChannel(request).then((ok) => { if (ok) onClose(); });
  };
  const toggle = (target: NotifyTargetView) => void onSaveTarget(targetInput(target, { enabled: !target.enabled }));
  const channelNotes = notes(kind, text);

  return (
    <Dialog
      description={subtitle(kind, text)}
      footer={
        <div className="notify-dialog-footer">
          {hasSecret && channel.secret_saved ? (
            <Button
              disabled={busy}
              onClick={() => void onSaveChannel({ config, enabled, secret: null, clear_secret: true })}
              size="sm"
              variant="danger-outline"
            >
              {text("Clear the channel secret", "清除渠道密钥")}
            </Button>
          ) : (
            <span>{text("Write-only secrets encrypted with AES-256-GCM · leave empty to keep", "标“只写”的密钥 AES-256-GCM 加密存储 · 留空保留原值")}</span>
          )}
          <div className="notify-dialog-footer-btns">
            <Button onClick={onClose} variant="secondary">{text("Cancel", "取消")}</Button>
            <Button disabled={busy} onClick={save} variant="primary">{text("Save", "保存")}</Button>
          </div>
        </div>
      }
      onOpenChange={(nextOpen) => {
        if (!nextOpen) onClose();
      }}
      open
      title={kindName(kind, text)}
      width="lg"
    >
      <div className="notify-dialog-stack">
        <div className="notify-section-hd">
          <label className="check-inline">
            <input checked={enabled} onChange={(event) => setEnabled(event.target.checked)} type="checkbox" />
            <span>{text("Channel on", "启用渠道")}</span>
          </label>
        </div>

        {error ? (
          <div className="notice-banner notice-banner--danger" role="alert">
            <span>{notifyActionText(error, text)}</span>
          </div>
        ) : null}

        {channel.secret_unreadable ? (
          <div className="notice-banner notice-banner--warn" role="status">
            <span>{text("The saved channel secret cannot be decrypted (the master key changed); enter it again.", "已保存的渠道密钥无法解密（主密钥已更换），请重新填写。")}</span>
          </div>
        ) : null}

        <section aria-labelledby="notify-channel-settings" className="notify-section">
          <div className="notify-section-hd">
            <h3 className="notify-section-title" id="notify-channel-settings">
              {text("Channel settings", "渠道设置")} <span className="card-subtitle">· {text("how to send", "怎么发")}</span>
            </h3>
          </div>
          <ChannelFields config={config} onChange={setConfig} onSecret={setSecret} secret={secret} secretSaved={channel.secret_saved} text={text} />
          {!hasSecret && channelNotes[0] ? (
            <div className="notice-banner" role="note">
              <span>{channelNotes[0]}</span>
            </div>
          ) : null}
        </section>

        <section aria-labelledby="notify-channel-targets" className="notify-section">
          <div className="notify-section-hd">
            <h3 className="notify-section-title" id="notify-channel-targets">
              {text("Targets", "发送对象")}{" "}
              <span className="card-subtitle">
                · {targets.length === 0 ? text("none", "0 个") : text(`${targets.length} · ${targets.filter((t) => t.enabled).length} on`, `共 ${targets.length} 个 · 启用 ${targets.filter((t) => t.enabled).length} 个`)}
                {" · "}
                {text(`up to ${overview.max_targets_per_channel}`, `最多 ${overview.max_targets_per_channel} 个`)}
              </span>
            </h3>
            <Button
              disabled={targets.length >= overview.max_targets_per_channel}
              onClick={() => setEdit("new")}
              size="sm"
              variant="secondary"
            >
              + {text(`Add a ${noun}`, `添加${noun}`)}
            </Button>
          </div>

          {targets.length === 0 ? (
            <div className="notice-banner" role="note">
              <span>{text(`No ${noun} yet. This channel sends nothing until one is added.`, `还没有${noun}。添加后这个渠道才会发出消息。`)}</span>
            </div>
          ) : (
            <table className="data-table">
              <thead>
                <tr>
                  <th>{text("On", "启用")}</th>
                  <th>{text("Name", "名称")}</th>
                  <th>{kind === "email" ? text("Mailbox", "邮箱") : kind === "feishu" || kind === "dingtalk" ? "Webhook" : text("Phone", "手机号")}</th>
                  <th>{text("Receives", "接收严重度")}</th>
                  <th>{text("Latest delivery", "最近投递")}</th>
                  <th className="num">{text("Actions", "操作")}</th>
                </tr>
              </thead>
              <tbody>
                {targets.map((target) => (
                  <tr className={target.target_id === edit ? "is-selected" : undefined} key={target.target_id}>
                    <td>
                      <input
                        aria-label={text(`Switch ${target.name}`, `启用 ${target.name}`)}
                        checked={target.enabled}
                        disabled={busy}
                        onChange={() => toggle(target)}
                        type="checkbox"
                      />
                    </td>
                    <td>
                      <strong>{target.name}</strong>{" "}
                      {target.secret_unreadable ? <span className="state-stamp state-stamp--warn">{text("Re-enter", "需重新填写")}</span> : null}
                    </td>
                    <td className="mono">{target.address_hint}</td>
                    <td>{severityText(target.min_severity, text)}</td>
                    <td>
                      <span className={target.last?.state === "failed" ? "delta-up" : undefined}>
                        {target.last ? [deliveryStateText(target.last.state, text), target.last.error ? errorText(target.last.error, text) : null, target.last.provider_code].filter(Boolean).join(" · ") : "—"}
                      </span>{" "}
                      <TestChip result={tests[target.target_id]} text={text} />
                    </td>
                    <td className="num">
                      <span className="notify-table-actions">
                        <Button disabled={busy || tests[target.target_id] === "running"} onClick={() => onTest(targetInput(target), target.target_id)} size="sm" variant="ghost">
                          {text("Test", "测试")}
                        </Button>
                        <Button onClick={() => setEdit(target.target_id)} size="sm" variant="secondary">
                          {text("Edit", "编辑")}
                        </Button>
                        {confirmDelete === target.target_id ? (
                          <Button disabled={busy} onClick={() => { setConfirmDelete(null); onDeleteTarget(target.target_id); }} size="sm" variant="danger">
                            {text("Confirm delete", "确认删除")}
                          </Button>
                        ) : (
                          <Button onClick={() => setConfirmDelete(target.target_id)} size="sm" variant="danger-outline">
                            {text("Delete", "删除")}
                          </Button>
                        )}
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}

          {edit !== null ? (
            <TargetForm
              busy={busy}
              key={edit}
              kind={kind}
              onCancel={() => setEdit(null)}
              onSubmit={(input) => void onSaveTarget(input).then((ok) => { if (ok) setEdit(null); })}
              onTest={(input) => onTest(input, "form")}
              saved={editing}
              testResult={tests.form}
              text={text}
            />
          ) : null}
        </section>

        {(hasSecret ? channelNotes : channelNotes.slice(1)).length > 0 ? (
          <div className="notice-banner" role="note">
            <span>{(hasSecret ? channelNotes : channelNotes.slice(1)).join(" ")}</span>
          </div>
        ) : null}
      </div>
    </Dialog>
  );
}
