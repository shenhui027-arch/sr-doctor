import { useEffect, useRef, type JSX } from "react";
import type { Diagnostic, SourceView } from "../../../../contracts/generated/doctor-contracts";
import { Button, StateStamp } from "../../design";
import { localizedEnum } from "../../i18n/enum-label";
import { useLocale } from "../../i18n/locale";
import { useSelection } from "../selection/selection-store";
import "./source.css";

export function sourceRefExists(sourceView: SourceView, sourceRef: string): boolean {
  return sourceView.source_kind === "text_profile"
    ? sourceView.plain_text_lines.some((line) => line.source_ref === sourceRef)
    : sourceView.json_tokens.some((token) => token.source_ref === sourceRef);
}

/**
 * Scrolls the matching Profile line row (or JSON token) to center and focuses its button.
 * Matches via attribute comparison rather than CSS selector interpolation so arbitrary
 * `sourceRef` strings (e.g. JSON pointers) never fail selector parsing.
 */
export function revealSourceTarget(container: ParentNode | null, sourceRef: string | null): boolean {
  if (!container || !sourceRef) return false;
  const buttons = container.querySelectorAll<HTMLElement>("[data-source-ref]");
  let match: HTMLElement | null = null;
  for (const candidate of buttons) {
    if (candidate.getAttribute("data-source-ref") === sourceRef) {
      match = candidate;
      break;
    }
  }
  if (!match) return false;
  const row = match.closest("tr") ?? match;
  row.scrollIntoView?.({ block: "center" });
  match.focus?.({ preventScroll: true });
  return true;
}

export function SourcePanel({ diagnostics, provenanceIndex, sourceView }: { diagnostics: Diagnostic[]; provenanceIndex: string[]; sourceView: SourceView }): JSX.Element {
  const { text } = useLocale();
  const { sourceRef, sourceRevealCount, selectSourceRef } = useSelection();
  const sectionRef = useRef<HTMLElement>(null);
  const textLines = sourceView.plain_text_lines;
  const jsonTokens = sourceView.json_tokens;
  const focusedLine = textLines.find((line) => line.source_ref === sourceRef);
  const focusedTokens = jsonTokens.filter((token) => token.source_ref === sourceRef);

  useEffect(() => {
    revealSourceTarget(sectionRef.current, sourceRef);
  }, [sourceRef, sourceRevealCount, sourceView]);

  return (
    <section aria-labelledby="source-title" className="source-panel" ref={sectionRef}>
      <div className="source-panel-bar">
        <strong className="field-label" id="source-title">{text("Related source", "相关原文")}</strong>
        {sourceView.truncation_state === "truncated" ? (
          <StateStamp state="partial" detail={localizedEnum(sourceView.truncation_state, text)} />
        ) : (
          <span className="badge-stamp">{localizedEnum(sourceView.truncation_state, text)}</span>
        )}
      </div>

      {sourceRef ? (
        <div aria-live="polite" className="notice-banner source-focus">
          {focusedLine ? (
            <div className="source-focus-inner">
              <strong>{text(`Line ${focusedLine.line_no}`, `第 ${focusedLine.line_no} 行`)}</strong>
              <pre className="sql-well">{focusedLine.text}</pre>
            </div>
          ) : focusedTokens.length ? (
            <div className="source-focus-inner">
              <strong>{text("Selected original value", "所选原始内容")}</strong>
              <pre className="sql-well">{focusedTokens.map((token) => token.text).join("")}</pre>
            </div>
          ) : (
            <span>{text("This reference is not present in the retained source. No content is inferred.", "保留的原文中没有这条记录，不推断缺失内容。")}</span>
          )}
        </div>
      ) : null}

      <div className="source-content" data-source-kind={sourceView.source_kind}>
        {sourceView.source_kind === "text_profile" && textLines.length > 0 ? (
          <table aria-label={text("Original Profile lines", "Profile 原文")} className="data-table mono source-lines-table">
            <tbody>
              {textLines.map((line, index) => {
                const active = line.source_ref === sourceRef;
                return (
                  <tr className={active ? "is-selected" : undefined} key={`${line.source_ref}-${index}`}>
                    <td className="source-line-num">
                      <button
                        aria-pressed={active}
                        className="row-pick-btn mono"
                        data-source-ref={line.source_ref}
                        onClick={() => selectSourceRef(line.source_ref)}
                        type="button"
                      >
                        L{line.line_no}
                      </button>
                    </td>
                    <td className="source-line-code"><code>{line.text}</code></td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        ) : null}

        {sourceView.source_kind === "json_profile" && jsonTokens.length > 0 ? (
          <pre className="sql-well source-tokens">
            {jsonTokens.map((token, index) => (
              <button
                className={token.source_ref === sourceRef ? "source-token is-selected" : "source-token"}
                data-source-ref={token.source_ref}
                key={`${token.source_ref}-${token.kind}-${index}`}
                onClick={() => selectSourceRef(token.source_ref)}
                type="button"
              >
                <code>{token.text}</code>
              </button>
            ))}
          </pre>
        ) : null}

        {((sourceView.source_kind === "text_profile" && textLines.length === 0) || (sourceView.source_kind === "json_profile" && jsonTokens.length === 0)) ? (
          <div className="state-box">
            <span>{text("No safe source lines or tokens were supplied for", "未提供安全来源行或 token：")} {localizedEnum(sourceView.source_kind, text)}</span>
          </div>
        ) : null}
      </div>

      <details className="disclosure technical-disclosure">
        <summary>{text("Parsing details and reference IDs (advanced)", "解析详情与引用编号（高级）")}</summary>
        <div className="detail-stack source-disclosure-body">
          {sourceRef ? <p className="source-selected-ref">{text("Selected reference", "所选引用")}：<code className="mono">{sourceRef}</code></p> : null}
          <FindingDiagnostics diagnostics={diagnostics} onSelectSourceRef={selectSourceRef} />
          <details className="disclosure provenance-index">
            <summary>{text(`Full reference index (${provenanceIndex.length})`, `全部引用索引（${provenanceIndex.length}）`)}</summary>
            <div className="source-ref-list">
              {provenanceIndex.map((ref, index) => (
                <Button className="mono" key={`${ref}-${index}`} onClick={() => selectSourceRef(ref)} size="sm" variant="ghost">{ref}</Button>
              ))}
            </div>
          </details>
          {sourceView.limitations.length > 0 ? (
            <div className="source-limitations">
              <div className="field-label">{text("Source limitations", "来源限制")}</div>
              <ul className="cmp-reasons">
                {sourceView.limitations.map((value) => <li key={value}>{localizedEnum(value, text)}</li>)}
              </ul>
            </div>
          ) : null}
        </div>
      </details>
    </section>
  );
}

function FindingDiagnostics({ diagnostics, onSelectSourceRef }: { diagnostics: Diagnostic[]; onSelectSourceRef(sourceRef: string): void }): JSX.Element {
  const { text } = useLocale();
  return (
    <div className="diagnostics">
      <div className="field-label">{text("Diagnostics", "诊断信息")}</div>
      {diagnostics.length === 0 ? (
        <p className="source-empty-note">{text("No diagnostics were supplied by the analysis.", "分析未提供诊断信息。")}</p>
      ) : (
        diagnostics.map((diagnostic, index) => (
          <article className="source-diagnostic" key={`${diagnostic.stage}-${diagnostic.code}-${index}`}>
            <strong className="mono">{diagnostic.code}</strong>
            <span className="source-diagnostic-meta">{diagnostic.stage} · {localizedEnum(diagnostic.severity, text)}</span>
            <p>{diagnostic.message_id}</p>
            {diagnostic.affected_ids.length > 0 ? <p>{text("Affected:", "受影响：")} {diagnostic.affected_ids.join(", ")}</p> : null}
            {diagnostic.source_refs.length > 0 ? (
              <div className="source-ref-list">
                {diagnostic.source_refs.map((ref, refIndex) => (
                  <Button className="mono" key={`${ref}-${refIndex}`} onClick={() => onSelectSourceRef(ref)} size="sm" variant="ghost">{ref}</Button>
                ))}
              </div>
            ) : null}
          </article>
        ))
      )}
    </div>
  );
}
