import type { JSX, ReactNode } from "react";
import type { ConnectionCapabilityState } from "../../../../contracts/generated/doctor-contracts";
import { lookupEnumLabel } from "../../i18n/enum-label";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import type { ConnectionState } from "./connection-state";

type Tone = "ok" | "guard" | "pending" | "blocked";
type Row = { name: string; state: string; tone: Tone };

const CAPABILITY_LABELS: Record<ConnectionCapabilityState, readonly [string, string]> = {
  available: ["Available", "可用"],
  forbidden: ["Not permitted", "无权限"],
  unavailable: ["Not available", "不可用"],
  unknown: ["Unknown", "未知"],
};

/**
 * Capability probe results (boards/01-connect.html). Every state comes from the live
 * session's probe; before a connection nothing is claimed ("probed on connect").
 * Runtime Top and remote background collection are product boundaries, not probes.
 */
export function CapabilityList({ state }: { state: ConnectionState }): JSX.Element {
  const { text } = useLocale();
  const session = state.phase !== "connecting" && (state.phase === "ready" || state.phase === "unsupported") ? state.session : null;
  const pending: Pick<Row, "state" | "tone"> = { state: text("Probed on connect", "待探测"), tone: "pending" };
  const probe = (value: ConnectionCapabilityState | undefined): Pick<Row, "state" | "tone"> =>
    session && value ? capabilityState(value, text) : pending;

  const versionRow: Pick<Row, "state" | "tone"> = session
    ? state.phase === "ready"
      ? { state: text("Version pack matched", "版本包匹配"), tone: "ok" }
      : { state: text("Version not supported", "版本不受支持"), tone: "blocked" }
    : pending;

  const rows: Row[] = [
    {
      name: session?.detected_version?.value ? `StarRocks ${session.detected_version.value}` : "StarRocks 3.1–4.1",
      ...versionRow,
    },
    { name: text("AuditLoader audit table", "审计表与语句过滤"), ...probe(session?.capabilities.audit) },
    {
      name: text("Profile list and fetch", "Profile 列表与拉取"),
      ...probe(session ? weakest(session.capabilities.profile_list, session.capabilities.profile_fetch) : undefined),
    },
    { name: text("Read-only plan check", "只读计划与 Advisor"), ...probe(session?.capabilities.explain_costs) },
    { name: text("Runtime Top · run once", "受控执行一次 SQL"), state: text("Confirm every run", "每次需二次确认"), tone: "guard" },
    {
      name: text("Remote background collection", "远端集群后台采集"),
      state: session?.tls_mode === "docker_loopback_plaintext"
        ? text("Collects while connected", "连接期间采集")
        : text("Requires G1 production pilot", "需通过 G1 生产试点"),
      tone: "guard",
    },
  ];

  const summaryTag = session
    ? `${state.phase === "ready" ? text("Connected", "已连接") : text("Limited", "受限")} · ${session.detected_version?.value ?? text("version unknown", "版本未知")} · ${text("read-only", "只读")}`
    : text("Not connected", "待连接探测");

  return (
    <div className="capability-probe-section">
      <div className="capability-probe-hd">
        <span className="field-label">{text("Capability probe", "能力探测")}</span>
        <span className="card-subtitle mono">{summaryTag}</span>
      </div>
      <table aria-label={text("Connection capability probe", "连接能力探测结果")} className="data-table capability-probe-table">
        <tbody>
          {rows.map((row) => (
            <tr key={row.name}>
              <td>{row.name}</td>
              <td className="num">{renderProbeBadge(row)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function renderProbeBadge(row: Row): ReactNode {
  switch (row.tone) {
    case "ok":
      return (
        <span className="status-inline">
          <span aria-hidden="true" className="status-dot status-dot--ok" />
          {row.state}
        </span>
      );
    case "guard":
      return <span className="state-stamp state-stamp--warn">{row.state}</span>;
    case "blocked":
      return <span className="state-stamp state-stamp--critical">{row.state}</span>;
    default:
      return <span className="state-stamp">{row.state}</span>;
  }
}

function capabilityState(value: ConnectionCapabilityState, text: LocalizedText): Pick<Row, "state" | "tone"> {
  const label = lookupEnumLabel(CAPABILITY_LABELS, value, text);
  switch (value) {
    case "available":
      return { state: label, tone: "ok" };
    case "forbidden":
    case "unavailable":
      return { state: label, tone: "blocked" };
    case "unknown":
    default:
      return { state: label, tone: "pending" };
  }
}

/** Both reads are needed; the less available one decides. */
function weakest(a: ConnectionCapabilityState, b: ConnectionCapabilityState): ConnectionCapabilityState {
  const order: ConnectionCapabilityState[] = ["forbidden", "unavailable", "unknown", "available"];
  const ia = order.indexOf(a);
  const ib = order.indexOf(b);
  return order[Math.min(ia < 0 ? 2 : ia, ib < 0 ? 2 : ib)];
}
