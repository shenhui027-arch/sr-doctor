import { useEffect, useState } from "react";
import type { JSX, ReactNode } from "react";
import { useLocale } from "../../i18n/locale";
import "./sql-text.css";

/**
 * One reading block for SQL text the backend returned: rendered verbatim (no
 * formatting or parsing), bounded with its own scroll, focusable so a keyboard
 * can scroll it, and copied exactly as shown. The copy label says what the text
 * is (full SQL, a template or a returned prefix), never more.
 */
export function SqlText({ children, className, copyLabel, id, label, note, value }: {
  /** Marked-up rendering of exactly `value` (e.g. a highlighted excerpt); `value` is what is copied. */
  children?: ReactNode;
  className?: string;
  copyLabel: string;
  id?: string;
  label: string;
  note?: ReactNode;
  value: string;
}): JSX.Element {
  const { text } = useLocale();
  const [copy, setCopy] = useState<"idle" | "copied" | "failed">("idle");
  useEffect(() => setCopy("idle"), [value]);
  async function copyValue(): Promise<void> {
    try {
      await navigator.clipboard.writeText(value);
      setCopy("copied");
    } catch {
      setCopy("failed");
    }
  }
  return <div className={`sql-text${className ? ` ${className}` : ""}`} id={id}>
    <div className="sql-text-bar">
      {note ? <span className="sql-text-note">{note}</span> : null}
      <span aria-live="polite" className="sql-text-status" role="status">{copy === "copied" ? text("Copied", "已复制") : copy === "failed" ? text("Copy failed: clipboard unavailable", "复制失败：剪贴板不可用") : ""}</span>
      <button className="sql-text-copy" onClick={() => { void copyValue(); }} type="button">{copyLabel}</button>
    </div>
    <pre aria-label={label} tabIndex={0}><code>{children ?? value}</code></pre>
  </div>;
}

/**
 * A one-line SQL summary (ellipsis) with an explicit Expand button that opens
 * the same text as a SqlText block in place. Clicks inside never reach a
 * selectable row or label around it. `truncated` marks text the backend
 * returned as a prefix: the line ends in "…" and the block says so.
 */
export function ExpandableSqlLine({ copyLabel, id, label, lineClassName, note, truncated = false, value }: {
  copyLabel: string;
  id: string;
  label: string;
  lineClassName: string;
  note?: ReactNode;
  truncated?: boolean;
  value: string;
}): JSX.Element {
  const { text } = useLocale();
  const [open, setOpen] = useState(false);
  useEffect(() => setOpen(false), [value]);
  return <div className={`sql-line${open ? " open" : ""}`} onClick={(event) => event.stopPropagation()} onKeyDown={(event) => event.stopPropagation()}>
    <div className="sql-line-head">
      <div className={lineClassName} title={open ? undefined : value}>{value}{truncated ? "…" : ""}</div>
      <button aria-controls={open ? id : undefined} aria-expanded={open} aria-label={`${open ? text("Collapse", "收起") : text("Expand", "展开")} · ${label}`} className="sql-line-toggle" onClick={() => setOpen((current) => !current)} type="button">{open ? text("Collapse", "收起") : text("Expand", "展开")}</button>
    </div>
    {open ? <SqlText copyLabel={copyLabel} id={id} label={label} note={truncated ? <>{text("Sample truncated: only a prefix was returned", "样本已截断：只返回了前缀")}{note ? <> · {note}</> : null}</> : note} value={value} /> : null}
  </div>;
}

/**
 * The show/hide control for a SQL block. While the SQL loads it stays
 * focusable (aria-disabled, not disabled), so keyboard focus is not dropped,
 * and it names the block it controls once that block exists.
 */
export function SqlToggleButton({ className = "button secondary", controls, disabled = false, expanded, hideLabel, loading, onToggle, showLabel, title }: {
  className?: string;
  controls: string;
  disabled?: boolean;
  expanded: boolean;
  hideLabel?: string;
  loading: boolean;
  onToggle(): void;
  showLabel?: string;
  title?: string;
}): JSX.Element {
  const { text } = useLocale();
  return <button
    aria-busy={loading || undefined}
    aria-controls={expanded ? controls : undefined}
    aria-disabled={loading || undefined}
    aria-expanded={expanded}
    className={className}
    disabled={disabled}
    onClick={() => { if (!loading) onToggle(); }}
    title={title}
    type="button"
  >{loading ? text("Loading SQL…", "SQL 加载中…") : expanded ? hideLabel ?? text("Hide SQL", "收起 SQL") : showLabel ?? text("View SQL", "查看 SQL")}</button>;
}
