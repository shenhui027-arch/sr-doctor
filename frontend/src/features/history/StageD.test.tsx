import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { AlertView, AuditMetricFact, GovernanceItem, HistoryAlertsResponse } from "../../../../contracts/generated/doctor-contracts";
import { LocaleProvider, type LocalizedText } from "../../i18n/locale";
import { AlertDetailDrawer, AlertsPanel, alertStateText, alertTitle } from "./AlertsPanel";
import { GovernanceItemDrawer, GovernancePanel, governanceStateText, metricName, originText, outcomeText } from "./GovernancePanel";
import { governanceActionText, settleNote } from "./MonitoringWorkspaces";

const english = (value: string): string => value;
const chinese = (_english: string, value: string): string => value;
const ms = (value: number): AuditMetricFact => ({ metric_id: "query_time", input: { state: "text", value: String(value) }, state: "exact", parser_state: "exact", canonical_number: { kind: "integer", value: `${value}000000`, scale: null }, canonical_unit: "ns" });
const alert = (id: string, state: AlertView["state"], relative: string | null): AlertView => ({
  alert_id: id, target: "127.0.0.1:9030", template_key: `k${id}`, template: "SELECT a FROM t WHERE id = ?", kind: "official_digest", state,
  muted_until_seconds: state === "muted" ? 1_800_090_000 : null, first_seen_seconds: 1_800_000_000, last_seen_seconds: 1_800_003_600, resolved_seconds: state === "resolved" ? 1_800_007_200 : null,
  severity: "high", confidence: "medium", evidence: "complete", workload: "increased", baseline_median: ms(relative === null ? 0 : 100), current_median: ms(250),
  change: { direction: "up", magnitude: ms(relative === null ? 250 : 150), relative_percent: relative }, bucket_start_seconds: 1_800_000_000, exemplar_query_id: "3b7e19c2-8a10",
  comparability: { engine_version: "3.5.14", rule_version: "sr-doctor-fingerprint.v1" }, rule_changed: false, governance_item_id: null,
});
const alerts: HistoryAlertsResponse = {
  target: "127.0.0.1:9030",
  alerts: [alert("1", "open", "150.0"), alert("2", "resolved", null)],
  counts: { open: "1", acknowledged: "0", resolved: "1", muted: "0", rule_changed: "0" },
  channels: [{ name: "in_app", available: true }, { name: "email", available: true }, { name: "feishu", available: false }, { name: "aliyun_sms", available: false }],
  rule: { min_severity: "medium", mute_hours: 24, current_hours: 3, baseline_hours: 168, calibrated: false },
  limitations: [],
};
const noop = () => undefined;
const renderAlerts = (selected: string | null) => renderToStaticMarkup(<AlertsPanel alerts={alerts} busy={false} onAction={noop} onResolveRuleChanged={noop} onAddToQueue={noop} onOpenItem={noop} onOpenTrends={noop} onSelect={noop} selectedId={selected} text={english} />);
const renderAlertDrawer = (value: AlertView, text: LocalizedText = english) => {
  const drawer = (
    <AlertDetailDrawer
      alert={value}
      busy={false}
      muteHours={alerts.rule.mute_hours}
      onAction={noop}
      onAddToQueue={noop}
      onClose={noop}
      onOpenItem={noop}
      onOpenTrends={noop}
      open
      text={text}
    />
  );
  return renderToStaticMarkup(text === chinese ? <LocaleProvider initialLocale="zh">{drawer}</LocaleProvider> : drawer);
};

const item = (state: GovernanceItem["state"], allowed: GovernanceItem["allowed_transitions"], pinned: boolean): GovernanceItem => ({
  item_id: "7", target: "127.0.0.1:9030", template_key: "kg", template: "SELECT g FROM t WHERE name = ?", kind: "official_digest", title: "Slow name lookup", origin: "trends", alert_id: null,
  state, created_seconds: 1_800_000_000, updated_seconds: 1_800_000_100, change_seconds: state === "pending_verification" ? 1_800_000_200 : null,
  snapshot: pinned ? { pinned_seconds: 1_800_000_050, template: "SELECT g FROM t WHERE name = ?", window_start_seconds: 1_799_913_600, window_end_seconds: 1_800_003_600, comparability: { engine_version: "3.5.14", rule_version: "v1" }, executions: "420", valid_samples: "420", query_time_median: ms(250), scan_bytes_median: null, cpu_cost_median: null, exemplar_query_id: "g-1", verdict: null } : null,
  verification: state === "pending_verification" ? { verified_seconds: 1_800_003_000, after_start_seconds: 1_800_003_600, after_end_seconds: 1_800_007_200, outcome: { state: "insufficient_samples" }, metrics: [{ metric: "query_time", before: ms(250), after: ms(150), change: { direction: "down", magnitude: ms(100), relative_percent: "-40.0" } }], executions_before: "420", executions_after: "4", confidence: null, evidence: "partial", limitations: [] } : null,
  events: [{ at_seconds: 1_800_000_000, kind: "created", from: null, to: "new", actor: "local_user", note: null }],
  allowed_transitions: allowed,
});
const renderDrawer = (value: GovernanceItem) =>
  renderToStaticMarkup(<GovernanceItemDrawer busy={false} item={value} onCommand={noop} onOpenTrends={noop} open text={english} />);
const renderItem = (value: GovernanceItem) =>
  renderToStaticMarkup(
    <>
      <GovernancePanel busy={false} governance={{ items: [value], limitations: [] }} onCommand={noop} onOpenTrends={noop} onSelect={noop} selectedId="7" text={english} />
      <GovernanceItemDrawer busy={false} item={value} onCommand={noop} onOpenTrends={noop} open text={english} />
    </>,
  );

describe("alerts (design board 06)", () => {
  it("counts states, renders the list with no dialog on load and folds rules and outbound channel status into the footer", () => {
    const html = renderAlerts("1");
    expect(html).toContain('<span>New</span><span class="seg-count">1</span>');
    expect(html).toContain("Outbound notification status");
    expect(html).toContain("<dt>Email · SMTP</dt>");
    expect(html).toContain(">Enabled<");
    expect(html.match(/Off · set up under Settings › Notifications/g)).toHaveLength(2);
    expect(html).toContain("Too few samples or a version change raises no alert");
    expect(html).toMatch(/<details class="disclosure alert-rules-channels"><summary>Alert rules and notification channels<\/summary>/);
    expect(html).not.toMatch(/<details[^>]*\bopen\b/);
    expect(html).not.toContain('role="dialog"');
    expect(html).toContain('aria-haspopup="dialog"');
    expect(html).not.toContain("aria-pressed");
  });

  it("offers only the actions the state allows in the right-hand detail drawer", () => {
    const drawer = renderAlertDrawer(alerts.alerts[0]);
    expect(drawer).toContain('role="dialog"');
    expect(drawer).toContain(">Acknowledge<");
    expect(drawer).toContain(">Mute 24 hours<");
    expect(drawer).toContain(">Add to governance queue<");
    expect(drawer).not.toContain(">Reopen<");
    expect(drawer).toContain('class="evidence-meter ui-meter" data-level="complete"');
  });

  it("opens on all alerts when nothing is new", () => {
    const quiet = { ...alerts, alerts: [alert("3", "acknowledged", "50.0")], counts: { open: "0", acknowledged: "1", resolved: "0", muted: "0", rule_changed: "0" } };
    const html = renderToStaticMarkup(<AlertsPanel alerts={quiet} busy={false} onAction={noop} onResolveRuleChanged={noop} onAddToQueue={noop} onOpenItem={noop} onOpenTrends={noop} onSelect={noop} selectedId={null} text={english} />);
    expect(html).toMatch(/role="tab" aria-selected="true"[^>]*><span aria-hidden="true" class="seg-tab-pill"><\/span><span class="seg-tab-content"><span>All<\/span><span class="seg-count">1<\/span><\/span><\/button>/);
    expect(renderAlertDrawer(quiet.alerts[0])).toContain(">Mute 24 hours<");
  });

  it("flags alerts from older rules with one sentence + InfoTip and confirms manual resolution via Dialog", () => {
    expect(renderAlerts("1")).not.toContain("rule-changed");
    const stale = { ...alerts, alerts: [{ ...alert("4", "open", "80.0"), rule_changed: true }], counts: { ...alerts.counts, rule_changed: "1" } };
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><AlertsPanel alerts={stale} busy={false} onAction={noop} onResolveRuleChanged={noop} onAddToQueue={noop} onOpenItem={noop} onOpenTrends={noop} onSelect={noop} selectedId="4" text={chinese} /></LocaleProvider>);
    expect(html).toContain("有 1 条旧规则告警需手动确认关闭。");
    expect(html).toContain('aria-label="为什么旧规则告警需手动关闭"');
    expect(html).toContain(">全部标为已解决<");
    expect(html).toContain("规则已变更");
    expect(renderAlertDrawer(stale.alerts[0], chinese)).toContain("规则已变更，无法比较");

    const dialogHtml = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><AlertsPanel alerts={stale} busy={false} initialConfirmResolveOpen onAction={noop} onResolveRuleChanged={noop} onAddToQueue={noop} onOpenItem={noop} onOpenTrends={noop} onSelect={noop} selectedId="4" text={chinese} /></LocaleProvider>);
    expect(dialogHtml).toContain("确认关闭旧规则告警");
    expect(dialogHtml).toContain(">确认标为已解决<");
  });

  it("never invents a percentage for a 0 baseline", () => {
    expect(alertTitle(alerts.alerts[0], english)).toBe("Median query time 100 ms → 250 ms (+150.0%)");
    expect(alertTitle(alerts.alerts[1], english)).toBe("Median query time 0 ns → 250 ms (baseline 0, no percentage)");
  });
});

describe("governance queue (design board D)", () => {
  it("opens the drawer of the item an alert asked for, and only that one", () => {
    const governance = { items: [item("new", ["triaged", "wont_fix"], false)], limitations: [] };
    const render = (openItemId: string | null) => renderToStaticMarkup(
      <GovernancePanel busy={false} governance={governance} onCommand={noop} onOpenTrends={noop} onSelect={noop} openItemId={openItemId} selectedId="7" text={english} />,
    );
    expect(render("7")).toContain('role="dialog"');
    expect(render(null)).not.toContain('role="dialog"');
    // An item no longer listed opens nothing.
    expect(render("99")).not.toContain('role="dialog"');
  });

  it("renders the queue with no detail card and no dialog on load", () => {
    const html = renderToStaticMarkup(
      <GovernancePanel
        busy={false}
        governance={{ items: [item("new", ["triaged", "wont_fix"], false)], limitations: [] }}
        onCommand={noop}
        onOpenTrends={noop}
        onSelect={noop}
        selectedId="7"
        text={english}
      />,
    );
    expect(html).toContain("Governance queue");
    expect(html).toContain("Slow name lookup");
    expect(html).toContain('aria-selected="true"');
    expect(html).not.toContain("alerts-split");
    expect(html).not.toContain("card--detail");
    expect(html).not.toContain('role="dialog"');
    expect(html).not.toContain("modal-card");
    expect(html).not.toContain("Item #7");
    expect(html).not.toContain(">Pin baseline<");
  });

  it("renders only the server's allowed transitions and never a manual close", () => {
    const html = renderDrawer(item("new", ["triaged", "wont_fix"], false));
    expect(html).toContain("Item #7");
    expect(html).toContain(">Mark triaged<");
    expect(html).toContain(">Won&#x27;t fix<");
    expect(html).toContain(">Pin baseline<");
    expect(html).not.toMatch(/mark (as )?verified/i);
    expect(html).not.toContain("Verify the effect");
  });

  it("lets only the comparator move a pending item and shows a no-verdict result honestly", () => {
    const html = renderDrawer(item("pending_verification", ["in_progress", "wont_fix"], true));
    expect(html).toContain(">Verify the effect<");
    expect(html).toContain("Collecting samples · no verdict yet");
    expect(html).toContain("An item closes only when verification shows an improvement");
    expect(html).not.toContain(">Pin baseline<");
    expect(html).toContain("−100 ms · −40.0%");
  });

  it("asks for a pinned snapshot before a change can be recorded", () => {
    const html = renderDrawer(item("in_progress", ["wont_fix"], false));
    expect(html).toContain('<button class="btn btn-primary btn--sm" type="button" disabled="">Record the change and wait for verification</button>');
    expect(html).toContain("Pin a baseline first.");
    const zhHtml = renderToStaticMarkup(
      <LocaleProvider initialLocale="zh">
        <GovernanceItemDrawer
          busy={false}
          item={item("in_progress", ["wont_fix"], false)}
          onCommand={noop}
          onOpenTrends={noop}
          open
          text={chinese}
        />
      </LocaleProvider>,
    );
    expect(zhHtml).toContain("治理项 #7");
    expect(zhHtml).toContain("尚未钉住基线快照");
    expect(zhHtml).toContain("仅验证确认改善后自动关闭");
  });
});

describe("governance drafts and copy (audit V1-15, V1-18)", () => {
  const two = (selectedId: string, notes: Record<string, string>) => {
    const first = item("in_progress", ["wont_fix"], true);
    const second = { ...first, item_id: "8", title: "Other lookup" };
    const selected = [first, second].find((entry) => entry.item_id === selectedId) ?? first;
    return renderToStaticMarkup(
      <GovernanceItemDrawer busy={false} item={selected} notes={notes} onCommand={noop} onOpenTrends={noop} open text={english} />,
    );
  };

  it("keeps a change note with its own item", () => {
    expect(two("7", { "7": "added a partition filter" })).toContain(">added a partition filter</textarea>");
    expect(two("8", { "7": "added a partition filter" })).not.toContain("added a partition filter");
  });

  it("drops a draft only after the change is recorded", () => {
    const notes = { "7": "a", "8": "b" };
    expect(settleNote(notes, "7", false)).toBe(notes);
    expect(settleNote(notes, "7", true)).toEqual({ "8": "b" });
  });

  it("says the change happens outside this tool, not outside the cluster", () => {
    const current = item("in_progress", ["wont_fix"], true);
    const html = renderToStaticMarkup(
      <>
        <GovernancePanel busy={false} governance={{ items: [current], limitations: [] }} onCommand={noop} onOpenTrends={noop} onSelect={noop} selectedId="7" text={chinese} />
        <GovernanceItemDrawer busy={false} item={current} onCommand={noop} onOpenTrends={noop} open text={chinese} />
      </>,
    );
    expect(html).toContain("你在本工具外做了什么改动？");
    expect(html).not.toContain("集群外");
    expect(html).not.toContain("一天");
    expect(html).not.toContain("比较器");
  });

  it("localizes the channels that are off", () => {
    const html = renderToStaticMarkup(<AlertsPanel alerts={alerts} busy={false} onAction={noop} onResolveRuleChanged={noop} onAddToQueue={noop} onOpenItem={noop} onOpenTrends={noop} onSelect={noop} selectedId="1" text={chinese} />);
    expect(html.match(/未启用 · 在“设置 › 通知”里设置/g)).toHaveLength(2);
    expect(html).toContain("短信 · 阿里云");
    expect(html).not.toContain("Not available in this version");
  });
});

describe("refused actions (combined verification of PR #51)", () => {
  it("explains the governance uniqueness conflict instead of showing the code", () => {
    expect(governanceActionText("governance_item_already_open", english)).toBe("This template already has an open governance item; finish or close that one first.");
    expect(governanceActionText("something_else", english)).toBe("Request failed (something_else)");
  });
});

describe("unknown enum safety in AlertsPanel and GovernancePanel (correctness fix #6)", () => {
  it("renders unknown enum values in AlertsPanel and GovernancePanel without throwing or rendering undefined", () => {
    expect(alertStateText("future_alert_state" as never, english)).toBe("future_alert_state");
    expect(governanceStateText("future_gov_state" as never, english)).toBe("future_gov_state");
    expect(outcomeText({ state: "future_outcome" } as never, english)).toBe("future_outcome");
    expect(originText({ ...item("new", [], false), origin: "future_origin" as never }, english)).toBe("future_origin");
    expect(metricName("future_metric", english)).toBe("future_metric");

    const futureAlert: AlertView = {
      ...alerts.alerts[0],
      state: "future_alert_state" as never,
      severity: "future_sev" as never,
      confidence: "future_conf" as never,
      evidence: "future_ev" as never,
      kind: "future_kind" as never,
    };
    const alertsHtml = renderToStaticMarkup(
      <>
        <AlertsPanel
          alerts={{ ...alerts, counts: { ...alerts.counts, open: "0" }, alerts: [futureAlert] }}
          busy={false}
          onAction={noop}
          onAddToQueue={noop}
          onOpenItem={noop}
          onOpenTrends={noop}
          onResolveRuleChanged={noop}
          onSelect={noop}
          selectedId={futureAlert.alert_id}
          text={english}
        />
        <AlertDetailDrawer
          alert={futureAlert}
          busy={false}
          muteHours={alerts.rule.mute_hours}
          onAction={noop}
          onAddToQueue={noop}
          onClose={noop}
          onOpenItem={noop}
          onOpenTrends={noop}
          open
          text={english}
        />
      </>,
    );
    expect(alertsHtml).toContain("future_alert_state");
    expect(alertsHtml).toContain("future_sev");
    expect(alertsHtml).not.toContain("undefined");

    const futureGov: GovernanceItem = {
      ...item("pending_verification", ["future_transition" as never], true),
      state: "future_gov_state" as never,
      origin: "future_origin" as never,
      events: [{ at_seconds: 1_800_000_000, kind: "future_event" as never, from: null, to: "new", actor: "future_actor" as never, note: null }],
    };
    const govHtml = renderItem(futureGov);
    expect(govHtml).toContain("future_gov_state");
    expect(govHtml).toContain("future_origin");
    expect(govHtml).toContain("future_transition");
    expect(govHtml).not.toContain("undefined");
  });
});
