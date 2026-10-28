import type { JSX } from "react";

/** Narrow headers keep the first clause on screen; the rest stays in the accessible text (CSS `.header-detail`). */
export function HeaderStatusText({ value }: { value: string }): JSX.Element {
  const split = value.indexOf(" · ");
  if (split < 0) return <>{value}</>;
  return <>{value.slice(0, split)}<span className="header-detail">{value.slice(split)}</span></>;
}
