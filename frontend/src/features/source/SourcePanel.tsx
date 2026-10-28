import type { Diagnostic, SourceView } from "../../../../contracts/generated/doctor-contracts";
import type { JSX } from "react";
import { useLocale } from "../../i18n/locale";
import { localizedEnum } from "../../i18n/enum-label";
import { useSelection } from "../selection/selection-store";

export function sourceRefExists(sourceView: SourceView, sourceRef: string): boolean {
  return sourceView.source_kind === "text_profile"
    ? sourceView.plain_text_lines.some((line) => line.source_ref === sourceRef)
    : sourceView.json_tokens.some((token) => token.source_ref === sourceRef);
}

export function SourcePanel({ diagnostics, provenanceIndex, sourceView }: { diagnostics: Diagnostic[]; provenanceIndex: string[]; sourceView: SourceView }): JSX.Element {
  const { text } = useLocale();
  const { sourceRef, selectSourceRef } = useSelection();
  const textLines = sourceView.plain_text_lines;
  const jsonTokens = sourceView.json_tokens;
  const focusedLine = textLines.find((line) => line.source_ref === sourceRef);
  const focusedTokens = jsonTokens.filter((token) => token.source_ref === sourceRef);

  return <section className="panel source-panel" aria-labelledby="source-title">
    <div className="panel-heading"><div><p className="eyebrow">{text("Source evidence", "原文依据")}</p><h2 id="source-title">{text("Related source", "相关原文")}</h2></div><span className="state-label">{localizedEnum(sourceView.truncation_state, text)}</span></div>
    {sourceRef && <div className="source-focus" aria-live="polite">
      {focusedLine ? <><strong>{text(`Line ${focusedLine.line_no}`, `第 ${focusedLine.line_no} 行`)}</strong><pre>{focusedLine.text}</pre></> : focusedTokens.length ? <><strong>{text("Selected original value", "所选原始内容")}</strong><pre>{focusedTokens.map((token) => token.text).join("")}</pre></> : <p>{text("This reference is not present in the retained source. No content is inferred.", "保留的原文中没有这条记录，暂时没有可展示的依据。")}</p>}
    </div>}
    <div className="source-content" data-source-kind={sourceView.source_kind}>
      {sourceView.source_kind === "text_profile" && textLines.length > 0 && <ol className="source-lines">{textLines.map((line, index) => <li className={line.source_ref === sourceRef ? "focused" : ""} key={`${line.source_ref}-${index}`}><button data-source-ref={line.source_ref} onClick={() => selectSourceRef(line.source_ref)} type="button"><code>{line.line_no}</code><span>{line.text}</span></button></li>)}</ol>}
      {sourceView.source_kind === "json_profile" && jsonTokens.length > 0 && <pre className="source-tokens">{jsonTokens.map((token, index) => <button className={token.source_ref === sourceRef ? "focused" : ""} data-source-ref={token.source_ref} key={`${token.source_ref}-${token.kind}-${index}`} onClick={() => selectSourceRef(token.source_ref)} type="button"><code>{token.text}</code></button>)}</pre>}
      {((sourceView.source_kind === "text_profile" && textLines.length === 0) || (sourceView.source_kind === "json_profile" && jsonTokens.length === 0)) && <p className="muted">{text("No safe source lines or tokens were supplied for", "未提供安全来源行或 token：")} {sourceView.source_kind}。</p>}
    </div>
    <details className="technical-disclosure"><summary>{text("Parsing details and reference IDs (advanced)", "解析详情与引用编号（高级）")}</summary>
      {sourceRef && <p>{text("Selected reference", "所选引用")}：<code>{sourceRef}</code></p>}
      <FindingDiagnostics diagnostics={diagnostics} onSelectSourceRef={selectSourceRef} />
      <details className="provenance-index"><summary>{text(`Full reference index (${provenanceIndex.length})`, `全部引用索引（${provenanceIndex.length}）`)}</summary><div className="source-ref-list">{provenanceIndex.map((ref, index) => <button className="reference-button" key={`${ref}-${index}`} onClick={() => selectSourceRef(ref)} type="button">{ref}</button>)}</div></details>
      {sourceView.limitations.length > 0 && <div className="source-limitations"><p className="eyebrow">{text("Source limitations", "来源限制")}</p><ul>{sourceView.limitations.map((value) => <li key={value}>{value}</li>)}</ul></div>}
    </details>
  </section>;
}

function FindingDiagnostics({ diagnostics, onSelectSourceRef }: { diagnostics: Diagnostic[]; onSelectSourceRef(sourceRef: string): void }): JSX.Element {
  const { text } = useLocale();
  return <div className="diagnostics"><p className="eyebrow">{text("Diagnostics", "诊断信息")}</p>{diagnostics.length === 0 ? <p className="muted compact">{text("No diagnostics were supplied by the analysis.", "分析未提供诊断信息。")}</p> : diagnostics.map((diagnostic, index) => <article key={`${diagnostic.stage}-${diagnostic.code}-${index}`}><strong>{diagnostic.code}</strong><span>{diagnostic.stage} · {diagnostic.severity}</span><p>{diagnostic.message_id}</p>{diagnostic.affected_ids.length > 0 && <p>{text("Affected:", "受影响：")} {diagnostic.affected_ids.join(", ")}</p>}<div className="source-ref-list">{diagnostic.source_refs.map((ref, refIndex) => <button className="reference-button" key={`${ref}-${refIndex}`} onClick={() => onSelectSourceRef(ref)} type="button">{ref}</button>)}</div></article>)}</div>;
}
