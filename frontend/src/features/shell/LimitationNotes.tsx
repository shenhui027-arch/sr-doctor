import type { JSX } from "react";
import { limitationText } from "../../i18n/limitation-text";
import type { LocalizedText } from "../../i18n/locale";
import "./shell.css";

/**
 * Backend limitation codes as readable notes. Known codes are translated in
 * place (the code stays in the title); unregistered codes are listed only in a
 * collapsed technical-details block, never as raw identifiers in the main view.
 */
export function LimitationNotes({ codes, label, text }: { codes: readonly string[]; label?: string; text: LocalizedText }): JSX.Element | null {
  if (codes.length === 0) return null;
  const known = codes.flatMap((code) => {
    const readable = limitationText(code, text);
    return readable ? [{ code, readable }] : [];
  });
  const unknown = codes.filter((code) => limitationText(code, text) === null);
  return <div className="limitation-notes">
    <p className="limitation-notes-title">{label ?? text("Limitations", "限制")}</p>
    {known.length > 0 && <ul>{known.map(({ code, readable }) => <li key={code} title={code}>{readable}</li>)}</ul>}
    {unknown.length > 0 && <details className="limitation-notes-technical"><summary>{text("Technical details", "技术详情")} · {unknown.length}</summary><ul>{unknown.map((code) => <li key={code}><code>{code}</code></li>)}</ul></details>}
  </div>;
}
