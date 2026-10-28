import type { JSX } from "react";
import type { ConnectionCapabilityState } from "../../../../contracts/generated/doctor-contracts";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import type { ConnectionState } from "./connection-state";

type Tone = "ok" | "guard" | "plan" | "pending" | "blocked";
type Row = { name: string; state: string; tone: Tone };

const MARKS: Record<Tone, string> = { ok: "✓", guard: "!", plan: "·", pending: "·", blocked: "×" };

/**
 * Capability probe results (design board 05). Every state comes from the live
 * session's probe; before a connection nothing is claimed ("probed on
 * connect"). Runtime Top and the trend queue are product policy, not probes.
 */
export function CapabilityList({ state }: { state: ConnectionState }): JSX.Element {
  const { text } = useLocale();
  const session = state.phase !== "connecting" && (state.phase === "ready" || state.phase === "unsupported") ? state.session : null;
  const pending: Pick<Row, "state" | "tone"> = { state: text("Probed on connect", "连接后探测"), tone: "pending" };
  const probe = (value: ConnectionCapabilityState | undefined): Pick<Row, "state" | "tone"> => session && value ? capabilityState(value, text) : pending;
  const version = session
    ? state.phase === "ready"
      ? { state: text("Version pack matched", "版本包匹配"), tone: "ok" as const }
      : { state: text("Version not supported", "版本不受支持"), tone: "blocked" as const }
    : pending;
  const rows: Row[] = [
    { name: session?.detected_version?.value ? `StarRocks ${session.detected_version.value}` : "StarRocks 3.1～4.1", ...version },
    { name: text("AuditLoader audit table", "AuditLoader 审计表"), ...probe(session?.capabilities.audit) },
    { name: "ProfileList / get_query_profile()", ...probe(session ? weakest(session.capabilities.profile_list, session.capabilities.profile_fetch) : undefined) },
    { name: text("EXPLAIN COSTS plan check", "EXPLAIN COSTS 计划检查"), ...probe(session?.capabilities.explain_costs) },
    { name: text("Runtime Top · run once", "运行时 Top · 执行一次"), state: text("Confirm every run", "每次需显式确认"), tone: "guard" },
    { name: text("History, trends, alerts and governance", "历史、趋势、告警与治理"), state: session?.tls_mode === "docker_loopback_plaintext" ? text("Collects while connected", "连接期间采集") : text("This machine: collects while connected · remote: not enabled yet", "本机集群：连接期间采集 · 远端集群：尚未启用"), tone: "guard" },
  ];
  return <div className="capability-list">
    <div className="capability-list-heading">
      <span>{text("Capability probe", "能力探测结果")}</span>
      <span>{session ? text("This connection", "本次连接") : text("Not connected", "未连接")}</span>
    </div>
    <ul>
      {rows.map((row) => <li className={`capability-${row.tone}`} key={row.name}>
        <span aria-hidden="true" className="capability-mark">{MARKS[row.tone]}</span>
        <span className="capability-name">{row.name}</span>
        <span className="capability-state">{row.state}</span>
      </li>)}
    </ul>
  </div>;
}

function capabilityState(value: ConnectionCapabilityState, text: LocalizedText): Pick<Row, "state" | "tone"> {
  switch (value) {
    case "available": return { state: text("Available", "可用"), tone: "ok" };
    case "forbidden": return { state: text("Not permitted", "无权限"), tone: "blocked" };
    case "unavailable": return { state: text("Not available", "不可用"), tone: "blocked" };
    case "unknown": return { state: text("Unknown", "未知"), tone: "pending" };
  }
}

/** Both reads are needed; the less available one decides. */
function weakest(a: ConnectionCapabilityState, b: ConnectionCapabilityState): ConnectionCapabilityState {
  const order: ConnectionCapabilityState[] = ["forbidden", "unavailable", "unknown", "available"];
  return order[Math.min(order.indexOf(a), order.indexOf(b))];
}
