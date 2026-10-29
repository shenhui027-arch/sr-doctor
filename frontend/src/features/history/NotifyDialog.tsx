// Design board N1 dialog: one channel's settings (how to send) above its
// targets (to whom). Secret fields are write-only: empty means "keep the
// saved one"; the server never sends a secret back.
import { useEffect, useRef, useState } from "react";
import type { JSX, ReactNode } from "react";
import type {
  AliyunSmsRegion, EmailSecurity, NotifyChannelConfig, NotifyChannelKind, NotifyChannelSaveRequest, NotifyOverviewResponse,
  NotifySeverity, NotifyTargetInput, NotifyTargetView, TencentSmsRegion,
} from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";
import { TestChip, type TestResults } from "./NotifyPanel";
import { defaultConfig, deliveryStateText, emptyTarget, errorText, kindName, notifyActionText, severityText, targetInput, targetNoun } from "./notify-text";

/** `new` opens an empty target form; a target id opens that target. */
export type DialogFocus = { kind: NotifyChannelKind; edit: string | "new" | null };

function Field({ label, wide, hint, children }: { label: string; wide?: boolean; hint?: string; children: ReactNode }): JSX.Element {
  return <label className={wide ? "notify-field wide" : "notify-field"}>{label}{children}{hint && <span className="history-muted">{hint}</span>}</label>;
}

function secretPlaceholder(saved: boolean, text: LocalizedText): string {
  return saved ? text("Saved · leave empty to keep", "已保存 · 留空表示不修改") : text("Not saved", "未保存");
}

function subtitle(kind: NotifyChannelKind, text: LocalizedText): string {
  return {
    email: text("One SMTP server, many recipients. Prefer the mailbox's app password over its login password.", "一个 SMTP 服务器，发往多个收件人；建议使用邮箱的授权码而不是登录密码"),
    feishu: text("Each target is a custom bot in one group: group settings › bots › add a custom bot", "每个发送对象是一个群里的自定义机器人：群设置 › 群机器人 › 添加自定义机器人"),
    dingtalk: text("Each target is a custom bot in one group: group settings › bots › add › custom", "每个发送对象是一个群里的自定义机器人：群设置 › 机器人 › 添加机器人 › 自定义"),
    aliyun_sms: text("One AccessKey with an approved signature and template, many phone numbers", "一套 AccessKey 与已审核的签名、模板，发往多个手机号"),
    tencent_sms: text("One SecretId/SecretKey with an approved signature and template, many phone numbers", "一套 SecretId/SecretKey 与已审核的签名、模板，发往多个手机号"),
  }[kind];
}

function notes(kind: NotifyChannelKind, text: LocalizedText): string[] {
  return {
    email: [text("A recipient can be a personal mailbox or a mailing list.", "收件人可以是个人邮箱，也可以是邮件组地址。"), text("TLS modes always verify the server certificate; \"no encryption\" sends the password and message in clear text.", "加密方式始终校验服务器证书；选“不加密”时口令与告警内容明文经过网络。")],
    feishu: [text("Feishu has no channel settings; each group bot has its own webhook and signing secret. Only open.feishu.cn and open.larksuite.com are accepted.", "飞书没有渠道级设置：每个群机器人各有自己的 Webhook 与签名密钥。地址只接受 open.feishu.cn 与 open.larksuite.com。"), text("If the bot uses custom keywords, include SR Doctor.", "机器人若设了“自定义关键词”，请包含 SR Doctor。")],
    dingtalk: [text("DingTalk has no channel settings; each group bot has its own webhook and signing secret. Only oapi.dingtalk.com is accepted.", "钉钉没有渠道级设置：每个群机器人各有自己的 Webhook 与加签密钥。地址只接受 oapi.dingtalk.com。"), text("Choose \"sign\" as the security setting; with custom keywords, include SR Doctor.", "安全设置选“加签”；若用“自定义关键词”，请包含 SR Doctor。")],
    aliyun_sms: [text("Template example: SR Doctor: cluster ${cluster} has ${count} new regression alerts, highest severity ${level}.", "模板正文示例：SR Doctor：集群${cluster}新增${count}条性能回归告警，最高严重度${level}，请登录查看。"), text("Use a RAM user allowed to send SMS only.", "建议 RAM 子账号，只授予短信发送权限。")],
    tencent_sms: [text("The template takes variables in order: {1} cluster, {2} count, {3} severity.", "模板按顺序取变量：{1} 集群、{2} 条数、{3} 严重度。"), text("Use a sub-user allowed to send SMS only.", "建议子用户，只授予短信发送权限。")],
  }[kind];
}

function ChannelFields({ config, onChange, secret, onSecret, secretSaved, text }: {
  config: NotifyChannelConfig;
  onChange(config: NotifyChannelConfig): void;
  secret: string;
  onSecret(value: string): void;
  secretSaved: boolean;
  text: LocalizedText;
}): JSX.Element | null {
  const secretField = (label: string) => <Field label={`${label} · ${text("write-only", "只写")}`}>
    <input autoComplete="new-password" onChange={(event) => onSecret(event.target.value)} placeholder={secretPlaceholder(secretSaved, text)} type="password" value={secret} />
  </Field>;
  switch (config.kind) {
    case "email": return <div className="notify-grid">
      <Field label={text("SMTP host", "SMTP 主机")}><input onChange={(event) => onChange({ ...config, host: event.target.value.trim() })} placeholder="smtp.example.com" value={config.host} /></Field>
      <Field label={text("Port", "端口")}><input className="mono" inputMode="numeric" onChange={(event) => onChange({ ...config, port: Number(event.target.value.replace(/\D/g, "")) || 0 })} value={config.port || ""} /></Field>
      <Field label={text("Encryption", "加密方式")}>
        <select onChange={(event) => onChange({ ...config, security: event.target.value as EmailSecurity })} value={config.security}>
          <option value="tls">{text("SSL/TLS (465)", "SSL/TLS（465）")}</option>
          <option value="starttls">{text("STARTTLS (587)", "STARTTLS（587）")}</option>
          <option value="plain">{text("None (password and message in clear text)", "不加密（口令与内容明文传输）")}</option>
        </select>
      </Field>
      <Field label={text("Sender address", "发件人地址")}><input onChange={(event) => onChange({ ...config, from_address: event.target.value.trim() })} placeholder="alerts@example.com" value={config.from_address} /></Field>
      <Field label={text("Sender name", "发件人名称")}><input onChange={(event) => onChange({ ...config, from_name: event.target.value || null })} value={config.from_name ?? ""} /></Field>
      <Field label={text("Username (empty: no login)", "用户名（留空表示不登录）")}><input autoComplete="off" onChange={(event) => onChange({ ...config, username: event.target.value.trim() || null })} value={config.username ?? ""} /></Field>
      {secretField(text("Password / app password", "口令 / 授权码"))}
      {config.security === "plain" && <p className="notify-warning wide" role="note">{text("Without encryption the password and alert content cross the network in clear text.", "不加密时，口令与告警内容将明文经过网络。")}</p>}
    </div>;
    case "aliyun_sms": return <div className="notify-grid">
      <Field label="AccessKey ID"><input className="mono" onChange={(event) => onChange({ ...config, access_key_id: event.target.value.trim() })} placeholder="LTAI…" value={config.access_key_id} /></Field>
      {secretField("AccessKey Secret")}
      <Field label={text("Signature name", "签名名称")}><input onChange={(event) => onChange({ ...config, sign_name: event.target.value })} value={config.sign_name} /></Field>
      <Field label={text("Template code", "模板 CODE")}><input className="mono" onChange={(event) => onChange({ ...config, template_code: event.target.value.trim() })} placeholder="SMS_123456789" value={config.template_code} /></Field>
      <Field label={text("Region", "地域")}>
        <select onChange={(event) => onChange({ ...config, region: event.target.value as AliyunSmsRegion })} value={config.region}>
          <option value="cn_hangzhou">{text("cn-hangzhou (default)", "cn-hangzhou（默认）")}</option><option value="ap_southeast1">ap-southeast-1</option>
        </select>
      </Field>
      <Field label={text("At most per hour (whole channel)", "每小时最多条数（整个渠道）")}><input className="mono" inputMode="numeric" onChange={(event) => onChange({ ...config, hourly_limit: Number(event.target.value.replace(/\D/g, "")) || 0 })} value={config.hourly_limit || ""} /></Field>
    </div>;
    case "tencent_sms": return <div className="notify-grid">
      <Field label="SecretId"><input className="mono" onChange={(event) => onChange({ ...config, secret_id: event.target.value.trim() })} placeholder="AKID…" value={config.secret_id} /></Field>
      {secretField("SecretKey")}
      <Field label="SdkAppId"><input className="mono" inputMode="numeric" onChange={(event) => onChange({ ...config, sdk_app_id: event.target.value.trim() })} placeholder="1400…" value={config.sdk_app_id} /></Field>
      <Field label={text("Signature", "签名内容")}><input onChange={(event) => onChange({ ...config, sign_name: event.target.value })} value={config.sign_name} /></Field>
      <Field label={text("Template ID", "模板 ID")}><input className="mono" inputMode="numeric" onChange={(event) => onChange({ ...config, template_id: event.target.value.trim() })} value={config.template_id} /></Field>
      <Field label={text("Region", "地域")}>
        <select onChange={(event) => onChange({ ...config, region: event.target.value as TencentSmsRegion })} value={config.region}>
          <option value="ap_guangzhou">{text("ap-guangzhou (default)", "ap-guangzhou（默认）")}</option><option value="ap_beijing">ap-beijing</option><option value="ap_nanjing">ap-nanjing</option>
        </select>
      </Field>
      <Field label={text("At most per hour (whole channel)", "每小时最多条数（整个渠道）")}><input className="mono" inputMode="numeric" onChange={(event) => onChange({ ...config, hourly_limit: Number(event.target.value.replace(/\D/g, "")) || 0 })} value={config.hourly_limit || ""} /></Field>
    </div>;
    default: return null;
  }
}

function TargetForm({ kind, saved, onSubmit, onTest, onCancel, busy, testResult, text }: {
  kind: NotifyChannelKind;
  /** The target being edited; none when adding. */
  saved: NotifyTargetView | null;
  onSubmit(input: NotifyTargetInput): void;
  onTest(input: NotifyTargetInput): void;
  onCancel(): void;
  busy: boolean;
  testResult: TestResults[string] | undefined;
  text: LocalizedText;
}): JSX.Element {
  const [input, setInput] = useState<NotifyTargetInput>(() => saved ? targetInput(saved) : emptyTarget(kind));
  const isBot = kind === "feishu" || kind === "dingtalk";
  const isSms = kind === "aliyun_sms" || kind === "tencent_sms";
  const noun = targetNoun(kind, text);
  const set = (changes: Partial<NotifyTargetInput>) => setInput((current) => ({ ...current, ...changes }));
  return <form className="notify-target-form" onSubmit={(event) => { event.preventDefault(); onSubmit(input); }}>
    <strong>{saved ? text(`Edit ${noun}: ${saved.name}`, `编辑${noun}：${saved.name}`) : text(`Add a ${noun}`, `添加${noun}`)}</strong>
    <div className="notify-grid">
      <Field label={text("Name", "名称")}><input maxLength={40} onChange={(event) => set({ name: event.target.value })} placeholder={isBot ? text("e.g. DBA on-call group", "例如 DBA 值班群") : text("e.g. DBA on-call", "例如 DBA 值班")} required value={input.name} /></Field>
      <Field label={text("Receives", "接收严重度")}>
        <select onChange={(event) => set({ min_severity: event.target.value as NotifySeverity })} value={input.min_severity}>
          <option value="medium">{severityText("medium", text)}</option><option value="high">{severityText("high", text)}</option>
        </select>
      </Field>
      {kind === "email" && <Field label={text("Email address", "邮箱地址")} wide><input onChange={(event) => set({ address: event.target.value.trim() || null })} placeholder="name@example.com" required type="email" value={input.address ?? ""} /></Field>}
      {isSms && <Field hint={saved ? text(`Saved: ${saved.address_hint}. Leave empty to keep.`, `已保存：${saved.address_hint}，留空表示不修改`) : undefined} label={text("Phone number", "手机号")} wide>
        <input className="mono" inputMode="tel" onChange={(event) => set({ address: event.target.value.trim() || null })} placeholder={text("11-digit mobile, or +country code", "11 位手机号，或 +国家区号")} required={!saved} value={input.address ?? ""} />
      </Field>}
      {isBot && <>
        <Field hint={saved ? text(`Saved: ${saved.address_hint}`, `已保存：${saved.address_hint}`) : undefined} label={`${kind === "feishu" ? text("Webhook address", "Webhook 地址") : text("Webhook address (with access_token)", "Webhook 地址（含 access_token）")} · ${text("write-only", "只写")}`} wide>
          <input autoComplete="off" onChange={(event) => set({ webhook: event.target.value.trim() || null })} placeholder={saved ? secretPlaceholder(true, text) : kind === "feishu" ? "https://open.feishu.cn/open-apis/bot/v2/hook/…" : "https://oapi.dingtalk.com/robot/send?access_token=…"} required={!saved} type="password" value={input.webhook ?? ""} />
        </Field>
        <Field label={`${kind === "feishu" ? text("Signing secret (when the bot checks signatures)", "签名密钥（机器人开启“签名校验”时填写）") : text("Signing secret (starts with SEC)", "加签密钥（以 SEC 开头）")} · ${text("write-only", "只写")}`} wide>
          <input autoComplete="new-password" disabled={input.clear_signing_secret} onChange={(event) => set({ signing_secret: event.target.value.trim() || null })} placeholder={secretPlaceholder(Boolean(saved?.signing_secret_saved), text)} type="password" value={input.signing_secret ?? ""} />
        </Field>
        {saved?.signing_secret_saved && <label className="notify-check wide"><input checked={input.clear_signing_secret} onChange={(event) => set({ clear_signing_secret: event.target.checked, signing_secret: null })} type="checkbox" /> {text("Remove the saved signing secret", "删除已保存的签名密钥")}</label>}
      </>}
    </div>
    <div className="notify-actions">
      <button className="button accent" disabled={busy} type="submit">{saved ? text("Update", "更新") : text("Add", "添加")}</button>
      <button className="button secondary" disabled={busy || testResult === "running"} onClick={() => onTest(input)} type="button">{text("Send a test first", "先发一条测试")}</button>
      <button className="button text" onClick={onCancel} type="button">{text("Cancel", "取消")}</button>
      <TestChip result={testResult} text={text} />
    </div>
  </form>;
}

export function NotifyDialog({ focus, overview, busy, error, tests, onClose, onSaveChannel, onSaveTarget, onDeleteTarget, onTest, text }: {
  focus: DialogFocus;
  overview: NotifyOverviewResponse;
  busy: boolean;
  /** The last refused request's code. */
  error: string | null;
  tests: TestResults;
  onClose(): void;
  onSaveChannel(request: NotifyChannelSaveRequest): Promise<boolean>;
  onSaveTarget(input: NotifyTargetInput): Promise<boolean>;
  onDeleteTarget(targetId: string): void;
  /** `key` names where the result shows: a target id, or `form`. */
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
  const dialog = useRef<HTMLDivElement>(null);
  useEffect(() => { dialog.current?.focus(); }, []);
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
  return <div className="notify-overlay" onKeyDown={(event) => { if (event.key === "Escape") onClose(); }} role="presentation">
    <div aria-labelledby="notify-dialog-title" aria-modal="true" className="notify-dialog" ref={dialog} role="dialog" tabIndex={-1}>
      <div className="notify-dialog-head">
        <div>
          <h2 id="notify-dialog-title">{kindName(kind, text)}</h2>
          <span className="history-muted">{subtitle(kind, text)}</span>
        </div>
        <label className="notify-check"><input checked={enabled} onChange={(event) => setEnabled(event.target.checked)} type="checkbox" /> {text("Channel on", "启用渠道")}</label>
        <button aria-label={text("Close", "关闭")} className="notify-close" onClick={onClose} type="button">
          <svg aria-hidden="true" fill="none" height="18" stroke="currentColor" strokeLinecap="round" strokeWidth="2" viewBox="0 0 24 24" width="18"><path d="M6 6l12 12M18 6L6 18" /></svg>
        </button>
      </div>
      <div className="notify-dialog-body">
        {error && <p className="capability-error" role="alert">{notifyActionText(error, text)}</p>}
        {channel.secret_unreadable && <p className="notify-warning" role="status">{text("The saved channel secret cannot be decrypted (the master key changed); enter it again.", "已保存的渠道密钥无法解密（主密钥已更换），请重新填写。")}</p>}
        <section aria-labelledby="notify-channel-settings" className="notify-section">
          <h3 id="notify-channel-settings">{text("Channel settings", "渠道设置")} <span className="history-muted">{text("how to send", "怎么发")}</span></h3>
          <ChannelFields config={config} onChange={setConfig} onSecret={setSecret} secret={secret} secretSaved={channel.secret_saved} text={text} />
          {!hasSecret && <p className="history-muted">{notes(kind, text)[0]}</p>}
        </section>
        <section aria-labelledby="notify-channel-targets" className="notify-section">
          <div className="history-card-head">
            <h3 id="notify-channel-targets">{text("Targets", "发送对象")} <span className="history-muted">{targets.length === 0 ? text("none", "0 个") : text(`${targets.length} · ${targets.filter((t) => t.enabled).length} on`, `共 ${targets.length} 个 · 启用 ${targets.filter((t) => t.enabled).length} 个`)}</span></h3>
            <button className="button secondary" disabled={targets.length >= overview.max_targets_per_channel} onClick={() => setEdit("new")} type="button">+ {text(`Add a ${noun}`, `添加${noun}`)}</button>
          </div>
          {targets.length === 0 ? <p className="notify-empty">{text(`No ${noun} yet. This channel sends nothing until one is added.`, `还没有${noun}。添加后这个渠道才会发出消息。`)}</p> : <div className="history-table-wrap notify-target-table">
            <table className="history-table">
              <thead><tr>
                <th>{text("On", "启用")}</th><th>{text("Name", "名称")}</th><th>{kind === "email" ? text("Mailbox", "邮箱") : kind === "feishu" || kind === "dingtalk" ? "Webhook" : text("Phone", "手机号")}</th>
                <th>{text("Receives", "接收严重度")}</th><th>{text("Latest delivery", "最近投递")}</th><th className="num">{text("Actions", "操作")}</th>
              </tr></thead>
              <tbody>
                {targets.map((target) => <tr className={target.target_id === edit ? "selected" : undefined} key={target.target_id}>
                  <td><input aria-label={text(`Switch ${target.name}`, `启用 ${target.name}`)} checked={target.enabled} disabled={busy} onChange={() => toggle(target)} type="checkbox" /></td>
                  <td><strong>{target.name}</strong>{target.secret_unreadable && <span className="notify-chip warn">{text("Re-enter", "需重新填写")}</span>}</td>
                  <td className="mono">{target.address_hint}</td>
                  <td>{severityText(target.min_severity, text)}</td>
                  <td className={target.last?.state === "failed" ? "bad" : "muted"}>{target.last ? [deliveryStateText(target.last.state, text), target.last.error ? errorText(target.last.error, text) : null, target.last.provider_code].filter(Boolean).join(" · ") : "—"} <TestChip result={tests[target.target_id]} text={text} /></td>
                  <td className="num">
                    <button className="button text" disabled={busy || tests[target.target_id] === "running"} onClick={() => onTest(targetInput(target), target.target_id)} type="button">{text("Test", "测试")}</button>
                    <button className="button text" onClick={() => setEdit(target.target_id)} type="button">{text("Edit", "编辑")}</button>
                    {confirmDelete === target.target_id
                      ? <button className="button text danger-text" disabled={busy} onClick={() => { setConfirmDelete(null); onDeleteTarget(target.target_id); }} type="button">{text("Confirm delete", "确认删除")}</button>
                      : <button className="button text danger-text" onClick={() => setConfirmDelete(target.target_id)} type="button">{text("Delete", "删除")}</button>}
                  </td>
                </tr>)}
              </tbody>
            </table>
          </div>}
          {edit !== null && <TargetForm
            busy={busy}
            key={edit}
            kind={kind}
            onCancel={() => setEdit(null)}
            onSubmit={(input) => void onSaveTarget(input).then((ok) => { if (ok) setEdit(null); })}
            onTest={(input) => onTest(input, "form")}
            saved={editing}
            testResult={tests.form}
            text={text}
          />}
        </section>
        <ul className="notify-notes boxed">
          {(hasSecret ? notes(kind, text) : notes(kind, text).slice(1)).map((note) => <li key={note}>{note}</li>)}
          <li>{text(`At most ${overview.max_targets_per_channel} targets per channel; one failing target does not stop the others.`, `每个渠道最多 ${overview.max_targets_per_channel} 个发送对象；一个对象失败不影响同渠道的其他对象。`)}</li>
          <li>{text("Write-only fields are encrypted (AES-256-GCM) into the local history store; the page and API only say \"saved\". Leave them empty to keep them.", "标“只写”的字段保存后加密存入本机历史库（AES-256-GCM），界面与接口只显示“已保存”；编辑时留空表示不修改。")}</li>
        </ul>
      </div>
      <div className="notify-dialog-foot">
        {hasSecret && channel.secret_saved && <button className="button text danger-text" disabled={busy} onClick={() => void onSaveChannel({ config, enabled, secret: null, clear_secret: true })} type="button">{text("Clear the channel secret", "清除渠道密钥")}</button>}
        <span className="notify-spacer" />
        <button className="button secondary" onClick={onClose} type="button">{text("Cancel", "取消")}</button>
        <button className="button accent" disabled={busy} onClick={save} type="button">{text("Save", "保存")}</button>
      </div>
    </div>
  </div>;
}
