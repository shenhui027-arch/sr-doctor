import type { JSX } from "react";
import type { ConnectionCapabilityState, ConnectionSessionView } from "../../../../contracts/generated/doctor-contracts";
import { useLocale } from "../../i18n/locale";
import { InfoTip } from "../shell/ui";

const chips = [["audit", "AuditLoader"], ["profile_list", "ProfileList"], ["explain_costs", "EXPLAIN COSTS"]] as const;

/**
 * The session's probed capability states and its transport; nothing is
 * inferred here. Task 64: only what limits the user stays on screen (a
 * capability that is not available, a plaintext transport); the full list is
 * behind the info icon.
 */
export function QueryCapabilityChips({ session }: { session: ConnectionSessionView }): JSX.Element {
  const { text } = useLocale();
  const stateLabel: Record<ConnectionCapabilityState, [string, string]> = { available: ["available", "可用"], unavailable: ["unavailable", "不可用"], forbidden: ["forbidden", "已禁止"], unknown: ["unknown", "未知"] };
  const transport = session.tls_mode === "verify_full" ? "TLS verify-full" : session.tls_mode === "plaintext" ? text("Plaintext", "明文") : text("Loopback Docker · plaintext", "回环 Docker · 明文");
  const limited = chips.filter(([key]) => session.capabilities[key] !== "available");
  return <div className="query-capabilities">
    {limited.map(([key, label]) => <span className="query-capability-limited" key={key}>{`${label} ${text(...stateLabel[session.capabilities[key]])}`}</span>)}
    {session.tls_mode !== "verify_full" && <span className="query-capability-transport">{transport}</span>}
    <InfoTip label={text("Connection capabilities", "连接能力")} side="bottom">
      <ul className="query-capability-list">
        {chips.map(([key, label]) => <li key={key}>{`${label} ${text(...stateLabel[session.capabilities[key]])}`}</li>)}
        <li>{session.tls_mode === "verify_full" ? transport : `${transport} · ${text("password and SQL are not encrypted", "口令与 SQL 不加密传输")}`}</li>
      </ul>
    </InfoTip>
  </div>;
}
