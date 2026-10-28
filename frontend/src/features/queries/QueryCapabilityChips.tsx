import type { JSX } from "react";
import type { ConnectionCapabilityState, ConnectionSessionView } from "../../../../contracts/generated/doctor-contracts";
import { useLocale } from "../../i18n/locale";

const chips = [["audit", "AuditLoader"], ["profile_list", "ProfileList"], ["explain_costs", "EXPLAIN COSTS"]] as const;

/** The session's probed capability states and its transport (design board 01 toolbar); nothing is inferred here. */
export function QueryCapabilityChips({ session }: { session: ConnectionSessionView }): JSX.Element {
  const { text } = useLocale();
  const stateLabel: Record<ConnectionCapabilityState, [string, string]> = { available: ["available", "可用"], unavailable: ["unavailable", "不可用"], forbidden: ["forbidden", "已禁止"], unknown: ["unknown", "未知"] };
  const tone: Record<ConnectionCapabilityState, string> = { available: "ok", unavailable: "warn", forbidden: "warn", unknown: "dashed" };
  return <div className="query-capabilities">
    <span>{text("Connection capabilities", "连接能力")}</span>
    <ul aria-label={text("Connection capabilities", "连接能力")}>
      {chips.map(([key, label]) => { const state = session.capabilities[key]; return <li className={`chip ${tone[state]}`} key={key}>{`${label} ${text(...stateLabel[state])}`}</li>; })}
      <li className="chip">{session.tls_mode === "verify_full" ? "TLS verify-full" : session.tls_mode === "plaintext" ? text("Plaintext", "明文") : text("Loopback Docker · plaintext", "回环 Docker · 明文")}</li>
    </ul>
  </div>;
}
