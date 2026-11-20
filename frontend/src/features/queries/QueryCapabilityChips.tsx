import type { JSX } from "react";
import type { ConnectionCapabilityState, ConnectionSessionView } from "../../../../contracts/generated/doctor-contracts";
import { InfoTip } from "../../design";
import { lookupEnumLabel } from "../../i18n/enum-label";
import { useLocale } from "../../i18n/locale";

const chips = [
  ["audit", "AuditLoader"],
  ["profile_list", "ProfileList"],
  ["explain_costs", "EXPLAIN COSTS"],
] as const;

const STATE_LABELS: Record<ConnectionCapabilityState, readonly [string, string]> = {
  available: ["available", "可用"],
  unavailable: ["unavailable", "不可用"],
  forbidden: ["forbidden", "已禁止"],
  unknown: ["unknown", "未知"],
};

const TLS_MODE_LABELS: Record<ConnectionSessionView["tls_mode"], readonly [string, string]> = {
  verify_full: ["TLS verify-full", "TLS verify-full"],
  plaintext: ["Plaintext", "明文"],
  docker_loopback_plaintext: ["Loopback Docker · plaintext", "回环 Docker · 明文"],
};

/**
 * The session's probed capability states and its transport; nothing is
 * inferred here. Only what limits the user stays on screen (a capability that
 * is not available, or a plaintext transport); the full list is behind InfoTip.
 */
export function QueryCapabilityChips({ session }: { session: ConnectionSessionView }): JSX.Element {
  const { text } = useLocale();
  const transport = lookupEnumLabel(TLS_MODE_LABELS, session.tls_mode, text);
  const limited = chips.filter(([key]) => session.capabilities[key] !== "available");

  return (
    <div className="query-capabilities">
      {limited.map(([key, label]) => (
        <span className="state-stamp state-stamp--warn query-capability-limited" key={key}>
          {`${label} ${lookupEnumLabel(STATE_LABELS, session.capabilities[key], text)}`}
        </span>
      ))}
      {session.tls_mode !== "verify_full" && (
        <span
          className="status-inline query-capability-transport"
          title={text("Password and SQL are not encrypted in transit", "口令与 SQL 不加密传输")}
        >
          <span className="status-dot status-dot--warn" />
          {transport}
        </span>
      )}
      <InfoTip label={text("Connection capabilities", "连接能力")} side="bottom">
        <ul className="query-capability-list">
          {chips.map(([key, label]) => (
            <li key={key}>{`${label} ${lookupEnumLabel(STATE_LABELS, session.capabilities[key], text)}`}</li>
          ))}
          <li>
            {session.tls_mode === "verify_full"
              ? transport
              : `${transport} · ${text("password and SQL are not encrypted", "口令与 SQL 不加密传输")}`}
          </li>
        </ul>
      </InfoTip>
    </div>
  );
}
