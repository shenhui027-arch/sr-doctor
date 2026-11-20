import type { JSX, ReactNode } from "react";

/**
 * Two-line StarRocks counter display (spec §5.5):
 * Localized label on top, official counter name in 12px monospace below, no parentheses.
 */
export function MetricName(props: { label: ReactNode; official?: string }): JSX.Element {
  const { label, official } = props;
  return (
    <span className="metric-name">
      <span>{label}</span>
      {/* No second line when the label already is the official name. */}
      {official && official !== label ? <span className="metric-sub" title={official}>{official}</span> : null}
    </span>
  );
}
