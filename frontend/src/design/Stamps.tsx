import type { JSX, ReactNode } from "react";
import { useLocale } from "../i18n/locale";
import { InfoTip } from "./ui";

export type HonestState =
  | "missing"
  | "invalid"
  | "partial"
  | "overflow"
  | "unknown_version"
  | "not_comparable"
  | "unscored";

const HONEST_COPY: Record<HonestState, { symbol: string; en: string; zh: string; tone?: "warn" | "critical" }> = {
  missing: { symbol: "∅", en: "Missing · not 0", zh: "缺失 · 非 0" },
  invalid: { symbol: "!", en: "Invalid", zh: "无效", tone: "critical" },
  partial: { symbol: "◐", en: "Partial", zh: "部分", tone: "warn" },
  overflow: { symbol: "↑", en: "Overflow", zh: "溢出" },
  unknown_version: { symbol: "?", en: "Unknown version", zh: "版本未知" },
  not_comparable: { symbol: "≠", en: "Not comparable", zh: "不可比" },
  unscored: { symbol: "—", en: "Unscored", zh: "未评分" },
};

/**
 * Honest non-exact data state stamp (AGENTS §3, spec §5.3).
 * Uses a distinct symbol + dashed border so non-exact states are never confused with numeric `0`.
 */
export function StateStamp(props: { state: HonestState; detail?: ReactNode }): JSX.Element {
  const { state, detail } = props;
  const { text } = useLocale();
  const entry = HONEST_COPY[state];
  const label = `${entry.symbol} ${text(entry.en, entry.zh)}`;
  const stampClass = [
    "state-stamp",
    entry.tone ? `state-stamp--${entry.tone}` : "",
  ].filter(Boolean).join(" ");

  const stamp = (
    <span className={stampClass} data-honest-state={state}>
      {label}
    </span>
  );

  if (detail === undefined || detail === null) return stamp;
  return (
    <span className="state-stamp-wrap">
      {stamp}
      <InfoTip label={text(`Details for ${entry.en}`, `${entry.zh}说明`)}>{detail}</InfoTip>
    </span>
  );
}

const BADGE_COPY: Record<
  "planned" | "demo" | "official_digest" | "unofficial_fingerprint",
  { en: string; zh: string; className: string }
> = {
  planned: { en: "Planned", zh: "规划中 · Planned", className: "badge-stamp badge-stamp--planned" },
  demo: { en: "Demo / Mock cluster", zh: "Demo / Mock cluster", className: "badge-stamp badge-stamp--demo" },
  official_digest: { en: "Official digest", zh: "官方 digest", className: "badge-stamp" },
  unofficial_fingerprint: { en: "Unofficial fingerprint", zh: "非官方指纹", className: "badge-stamp badge-stamp--unofficial" },
};

export function BadgeStamp(props: { kind: "planned" | "demo" | "official_digest" | "unofficial_fingerprint" }): JSX.Element {
  const { text } = useLocale();
  const entry = BADGE_COPY[props.kind];
  return (
    <span className={entry.className} data-badge-kind={props.kind}>
      {text(entry.en, entry.zh)}
    </span>
  );
}
