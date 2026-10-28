import { useCallback, useEffect, useRef, useState, type JSX, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { createPortal } from "react-dom";
import type { AnalysisSnapshotReport, ConnectedQueryRef, ConnectionSessionView, GraphNode, OfficialProfileReadResponse, ProfileFetchResponse } from "../../../../contracts/generated/doctor-contracts";
import { useLocale } from "../../i18n/locale";
import { doctorApi, type ApiFailure } from "../../lib/api";
import { useSelection } from "../selection/selection-store";
import { admitOfficialProfileBinding } from "./official-profile-binding";
import { createOfficialProfileReader, type OfficialBinding, type OfficialReadState } from "./official-profile-read";

/**
 * App-owned current Doctor evidence. This is deliberately narrower than a
 * query-list selection: the binding helper requires this same fetched Profile
 * and report object before it ever makes a fixed official-analysis request.
 */
export type OfficialProfileContext = Readonly<{
  session: ConnectionSessionView | null;
  currentQuery: ConnectedQueryRef | null;
  evidence: ProfileFetchResponse | null;
  onSessionExpired?(): void;
  onReturnToQueries?(): void;
  onReconnect?(): void;
}>;

type Scope = "whole_query" | "plan_node";
export type OfficialDisplayState = Extract<OfficialReadState, { readonly status: "success" }> | {
  readonly status: "unavailable" | "failed";
  readonly binding: OfficialBinding;
  readonly error: ApiFailure;
  readonly failureBinding?: OfficialBinding;
};

function bindingKey(binding: OfficialBinding | null, report: AnalysisSnapshotReport): string {
  if (!binding) return `closed:${report.analysis_key}`;
  return JSON.stringify([
    report.analysis_key,
    binding.connection_id,
    binding.query_id,
    binding.identity,
    binding.node_ids,
  ]);
}

function currentPlanNode(report: AnalysisSnapshotReport, nodeId: string | null): GraphNode | null {
  if (!nodeId) return null;
  return report.graph_view.nodes.find((node) => node.node_id === nodeId) ?? null;
}

export function officialBindingForScope(
  context: OfficialProfileContext,
  report: AnalysisSnapshotReport,
  scope: Scope,
  node: GraphNode | null,
): OfficialBinding | null {
  // A node scope never means "best effort whole query". Losing a graph
  // selection must close the node range rather than silently broadening it.
  if (scope === "plan_node" && !node) return null;
  return admitOfficialProfileBinding({
    session: context.session,
    query: context.currentQuery,
    evidence: context.evidence,
    report,
    scope: scope === "plan_node" && node ? { kind: "plan_node", node } : { kind: "whole_query" },
  });
}

function rangeText(binding: OfficialBinding, text: (english: string, chinese: string) => string): string {
  return binding.node_ids.length === 0
    ? text("Entire Query", "整个 Query")
    : text(`Selected node [${binding.node_ids[0]}]`, `所选节点 [${binding.node_ids[0]}]`);
}

// Readable copy for the backend's typed official-analysis reasons; unknown ids
// show no sentence and stay in the technical details only.
const officialReasons: Record<string, [string, string]> = {
  official_profile_not_retained: ["StarRocks no longer keeps this Profile (it was evicted or expired). Run the query again with Profile enabled to read it.", "StarRocks 已不再保留该 Profile（已被淘汰或过期）。请开启 Profile 重新执行该查询后再读取。"],
  official_profile_empty: ["StarRocks returned an empty analysis for this Profile.", "StarRocks 对该 Profile 返回了空的解读结果。"],
  official_profile_permission_denied: ["The connected user is not allowed to run ANALYZE PROFILE.", "当前连接用户无权执行 ANALYZE PROFILE。"],
  official_profile_remote_unsupported: ["This StarRocks server does not support ANALYZE PROFILE here.", "该 StarRocks 服务端在此不支持 ANALYZE PROFILE。"],
  official_profile_lifecycle_unknown: ["The query's state is unknown, so its Profile is not read.", "查询状态未知，因此不读取其 Profile。"],
};

function officialReasonText(messageId: string, text: (english: string, chinese: string) => string): string | null {
  const reason = officialReasons[messageId];
  return reason ? text(...reason) : null;
}

function renderError(error: ApiFailure): JSX.Element {
  const details = Object.entries(error.details);
  return <dl className="official-profile-error-details">
    <div><dt>code</dt><dd><code>{error.code}</code></dd></div>
    <div><dt>stage</dt><dd><code>{error.stage}</code></dd></div>
    <div><dt>message_id</dt><dd><code>{error.message_id}</code></dd></div>
    <div><dt>recoverability</dt><dd><code>{error.recoverability}</code></dd></div>
    {details.map(([key, value]) => <div key={key}><dt>{key}</dt><dd><code>{value}</code></dd></div>)}
  </dl>;
}

/** Pure, text-only result rendering: the client never parses or normalizes the official artifact. */
export function OfficialProfileReadDetails({ state }: { state: OfficialDisplayState }): JSX.Element {
  const { text } = useLocale();
  if (state.status === "success") return <OfficialSuccess response={state.response} binding={state.binding} />;
  const errorBinding = state.failureBinding;
  return <section className={`official-profile-result official-profile-${state.status}`} aria-live="polite">
    <p className="official-profile-state">{state.status === "unavailable"
      ? text("Official analysis is unavailable for this range.", "该范围的官方解读不可用。")
      : text("Official analysis request failed.", "官方解读请求失败。")}</p>
    {officialReasonText(state.error.message_id, text) && <p className="official-profile-reason">{officialReasonText(state.error.message_id, text)}</p>}
    <details className="official-profile-technical"><summary>{text("Technical details", "技术详情")}</summary>{renderError(state.error)}</details>
    {errorBinding &&<p className="official-profile-failure-origin">{text("Recovery requirement originated from", "恢复要求来自")}: <code>{rangeText(errorBinding, text)}</code></p>}
  </section>;
}

export function officialScopeSwitchLocked(state: OfficialReadState): boolean {
  if (state.status === "loading") return true;
  if (state.status !== "unavailable" && state.status !== "failed") return false;
  if (state.error.recoverability === "restart") return true;
  if (state.error.code === "profile_unavailable" && state.error.details.limitation === "query_discovery_generation_expired") return true;
  // A whole-query typed unavailable has no more specific admitted scope in the
  // frozen UI. A node unavailable may move back to the already-known whole
  // range, which is an explicit input change rather than an implicit retry.
  return state.error.recoverability === "change_input" && state.binding.node_ids.length === 0;
}

function OfficialSuccess({ binding, response }: { binding: OfficialBinding; response: OfficialProfileReadResponse }): JSX.Element {
  const { text } = useLocale();
  return <section className="official-profile-result official-profile-success" aria-live="polite">
    <div className="official-profile-result-heading">
      <p className="official-profile-state">{text("Official ANALYZE PROFILE text", "官方 ANALYZE PROFILE 文本")}</p>
      <span>{rangeText(binding, text)}</span>
    </div>
    <pre aria-label={text("Official display text", "官方展示文本")} className="official-profile-display-text" tabIndex={0}><code>{response.display_text}</code></pre>
    <dl className="official-profile-metadata">
      <div><dt>{text("Database", "数据库")}</dt><dd>{response.query.database === null ? text("Source did not provide", "来源未提供") : <code>{response.query.database}</code>}</dd></div>
      <div><dt>{text("Read at", "读取时间")}</dt><dd><time dateTime={response.read_at_utc}>{response.read_at_utc}</time></dd></div>
      <div><dt>{text("Profile observed at", "Profile 观测时间")}</dt><dd>{response.profile_observed_at_utc === null ? text("Source did not provide", "来源未提供") : <time dateTime={response.profile_observed_at_utc}>{response.profile_observed_at_utc}</time>}</dd></div>
      <div><dt>{text("Detected version", "检测版本")}</dt><dd>{response.detected_version ? <code>{response.detected_version.value}</code> : text("Source did not provide", "来源未提供")}</dd></div>
    </dl>
    {response.limitations.length ? <EvidenceList label={text("Limitations", "限制")} values={response.limitations} /> : null}
    <details className="official-profile-raw-source">
      <summary>{text("Read details and raw text", "读取详情与原始文本")}</summary>
      <dl className="official-profile-metadata">
        <div><dt>{text("Kind", "类型")}</dt><dd><code>{response.kind}</code></dd></div>
        <div><dt>{text("Connection ID", "连接 ID")}</dt><dd><code>{response.query.connection_id}</code></dd></div>
        <div><dt>{text("Discovery generation", "发现代际")}</dt><dd><code>{response.query.discovery_generation ?? text("Not applicable · verified execution", "不适用 · 已验证执行")}</code></dd></div>
        {response.detected_version ? <div><dt>{text("Version source", "版本来源")}</dt><dd><code>{response.detected_version.source_ref}</code></dd></div> : null}
        <div><dt>{text("Display normalization", "展示规范化")}</dt><dd><code>{response.display_normalization}</code></dd></div>
      </dl>
      <EvidenceList label={text("Source references", "来源引用")} values={response.source_refs} />
      <pre aria-label={text("Raw official text", "原始官方文本")} tabIndex={0}><code>{response.text}</code></pre>
    </details>
  </section>;
}

function EvidenceList({ label, values }: { label: string; values: readonly string[] }): JSX.Element {
  return <section className="official-profile-evidence" aria-label={label}>
    <h4>{label}</h4>
    {values.length === 0 ? <p>—</p> : <ul>{values.map((value, index) => <li key={`${value}:${index}`}><code>{value}</code></li>)}</ul>}
  </section>;
}

/**
 * The stateful UI wrapper deliberately delegates every request and response
 * identity decision to the pre-existing reader. Scope changes only call open;
 * they never read raw Profile content and never issue an implicit request.
 */
/** `triggerSlot`: render the trigger there (the Doctor graph's view switch) instead of the explanatory entry row. */
export function OfficialProfilePanel({ context, report, triggerSlot = null }: { context: OfficialProfileContext; report: AnalysisSnapshotReport; triggerSlot?: HTMLElement | null }): JSX.Element | null {
  const { text } = useLocale();
  const { nodeId } = useSelection();
  const [open, setOpen] = useState(false);
  const [scope, setScope] = useState<Scope>("whole_query");
  const [, setRevision] = useState(0);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const selectedNode = currentPlanNode(report, nodeId);
  const wholeBinding = officialBindingForScope(context, report, "whole_query", null);
  const selectedBinding = officialBindingForScope(context, report, "plan_node", selectedNode);
  const binding = scope === "plan_node" ? selectedBinding : wholeBinding;
  const liveBinding = useRef<OfficialBinding | null>(null);
  const liveContext = useRef(context);
  liveBinding.current = binding;
  liveContext.current = context;
  const readerRef = useRef<ReturnType<typeof createOfficialProfileReader> | null>(null);
  if (!readerRef.current) {
    readerRef.current = createOfficialProfileReader({
      read: (request, signal) => doctorApi.readOfficialProfile(request, signal),
      getCurrentBinding: () => liveBinding.current,
      onChange: () => setRevision((revision) => revision + 1),
      onSessionExpired: () => liveContext.current.onSessionExpired?.(),
    });
  }
  const reader = readerRef.current;
  const scopeBindingKey = bindingKey(binding, report);
  const visible = reader.snapshot();

  useEffect(() => {
    if (!open) {
      reader.invalidate();
      return;
    }
    reader.open();
  }, [open, reader, scopeBindingKey]);

  useEffect(() => () => reader.invalidate(), [reader]);

  useEffect(() => {
    if (!open) return;
    window.requestAnimationFrame(() => closeRef.current?.focus({ preventScroll: true }));
  }, [open]);

  const close = useCallback(() => {
    reader.close();
    setOpen(false);
    window.requestAnimationFrame(() => triggerRef.current?.focus({ preventScroll: true }));
  }, [reader]);
  const openWholeQuery = useCallback(() => {
    setScope("whole_query");
    setOpen(true);
  }, []);
  const switchScope = useCallback((next: Scope) => {
    if (next === scope || (next === "plan_node" && !selectedBinding)) return;
    reader.invalidate();
    setScope(next);
  }, [reader, scope, selectedBinding]);
  const onPanelKeyDown = useCallback((event: ReactKeyboardEvent<HTMLElement>) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    close();
  }, [close]);

  // No ready/fetched same-evidence identity means no official-read entry at all.
  if (!wholeBinding) return null;
  const loading = visible.status === "loading";
  const scopeLocked = officialScopeSwitchLocked(visible);
  const selectedLabel = selectedBinding ? rangeText(selectedBinding, text) : text("Selected plan node unavailable", "所选计划节点不可用");
  const trigger = <button aria-controls="official-profile-dialog" aria-expanded={open} aria-haspopup="dialog" className="button secondary official-profile-open" onClick={openWholeQuery} ref={triggerRef} title={triggerSlot ? text("Check this graph against StarRocks' own ANALYZE PROFILE output for the same Profile.", "用 StarRocks 官方 ANALYZE PROFILE 对同一 Profile 的输出核对这张图。") : undefined} type="button">{text("Official reading", "官方解读")}</button>;
  return <>
    {triggerSlot ? createPortal(trigger, triggerSlot) : <div className="official-profile-entry">
      <span>{text("Check this graph against StarRocks' own ANALYZE PROFILE output for the same Profile.", "用 StarRocks 官方 ANALYZE PROFILE 对同一 Profile 的输出核对这张图。")}</span>
      {trigger}
    </div>}
    {open && <section aria-label={text("Official profile reading", "官方 Profile 解读")} aria-modal="false" className="official-profile-dialog" id="official-profile-dialog" onKeyDown={onPanelKeyDown} role="dialog" tabIndex={-1}>
      <header>
        <div>
          <p className="eyebrow">{text("Existing Profile evidence", "既有 Profile 证据")}</p>
          <h3>{text("Official reading", "官方解读")}</h3>
          <p className="official-profile-query"><span>{text("Query ID", "查询 ID")}</span><code>{wholeBinding.query_id}</code></p>
        </div>
        <button className="official-profile-return" onClick={close} ref={closeRef} type="button">{loading ? text("Cancel reading", "取消读取") : text("Return topology", "返回拓扑")}</button>
      </header>
      <div className="official-profile-scope" aria-label={text("Official reading scope", "官方解读范围")}>
        <button aria-pressed={scope === "whole_query"} disabled={scopeLocked} onClick={() => switchScope("whole_query")} type="button">{text("Entire Query", "整个 Query")}</button>
        <button aria-pressed={scope === "plan_node"} disabled={scopeLocked || !selectedBinding} onClick={() => switchScope("plan_node")} type="button">{selectedLabel}</button>
      </div>
      <p className="official-profile-boundary">{text("Reads existing Profile evidence. It does not execute SQL.", "读取既有 Profile 证据，不执行 SQL。")}</p>
      {visible.status === "closed" ? <p className="official-profile-state" role="status">{text("Current Doctor evidence changed. Reopen this panel to read the current range.", "当前 Doctor 证据已变化。请重新打开面板读取当前范围。")}</p> : null}
      {visible.status === "unread" ? <section className="official-profile-action"><p>{text("No official text has been read for this range.", "尚未读取该范围的官方文本。")}</p><button className="button accent" onClick={() => { void reader.read(); }} type="button">{text("Read official analysis", "读取官方解读")}</button></section> : null}
      {visible.status === "loading" ? <section className="official-profile-action" aria-busy="true" role="status"><p>{text("Reading official analysis…", "正在读取官方解读…")}</p><button className="button accent" disabled type="button">{text("Reading…", "读取中…")}</button></section> : null}
      {(visible.status === "success" || visible.status === "unavailable" || visible.status === "failed") && <>
        <OfficialActions context={context} reader={reader} state={visible} text={text} />
        <OfficialProfileReadDetails state={visible} />
      </>}
    </section>}
  </>;
}

function OfficialActions({ context, reader, state, text }: {
  context: OfficialProfileContext;
  reader: ReturnType<typeof createOfficialProfileReader>;
  state: OfficialDisplayState;
  text: (english: string, chinese: string) => string;
}): JSX.Element {
  if (state.status === "success") return <div className="official-profile-actions"><button className="button accent" onClick={() => { void reader.read(); }} type="button">{text("Read again", "重新读取")}</button></div>;
  if (state.error.recoverability === "retry_same") return <div className="official-profile-actions"><button className="button accent" onClick={() => { void reader.read(); }} type="button">{text("Retry reading", "重试读取")}</button></div>;
  if (state.error.recoverability === "restart") return <div className="official-profile-actions"><button className="button accent" onClick={context.onReconnect} type="button">{text("Reconnect", "重新连接")}</button></div>;
  return <div className="official-profile-actions"><button className="button accent" onClick={context.onReturnToQueries} type="button">{text("Return to query list", "返回查询列表")}</button></div>;
}
