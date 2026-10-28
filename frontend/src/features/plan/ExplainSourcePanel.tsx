import type { ExplainSnapshot, ExplainSourceLocator, GraphEdge, GraphNode, SourceLine } from "../../../../contracts/generated/doctor-contracts";
import { useEffect, useMemo, useRef, type JSX } from "react";
import { useLocale } from "../../i18n/locale";
import { useSelection } from "../selection/selection-store";

export type ExplainSourceFocus = {
  node: GraphNode | null;
  adjacentEdges: GraphEdge[];
  sourceRefs: string[];
  lines: SourceLine[];
  locators: ExplainSourceLocator[];
};

export function explainLocatorSourceRef(locator: ExplainSourceLocator): string {
  return locator.source_ref;
}

export function explainSourceRefs(snapshot: ExplainSnapshot): string[] {
  return [...new Set([
    ...snapshot.source_refs,
    ...snapshot.source_view.lines.map((line) => line.source_ref),
    ...snapshot.source_locators.map(explainLocatorSourceRef),
  ])];
}

/** Selection only filters existing DTO evidence; it never reconstructs source or adjacency. */
export function explainSourceFocus(snapshot: ExplainSnapshot, nodeId: string | null): ExplainSourceFocus {
  const node = nodeId === null ? null : snapshot.graph_view.nodes.find((candidate) => candidate.node_id === nodeId) ?? null;
  const adjacentEdges = node === null ? [] : snapshot.graph_view.edges.filter((edge) => edge.from === node.node_id || edge.to === node.node_id);
  const selectedRefs = node === null ? snapshot.source_refs : [
    ...node.source_refs,
    ...adjacentEdges.flatMap((edge) => edge.evidence_refs),
  ];
  const sourceRefs = [...new Set([...selectedRefs, ...snapshot.version.source_refs])];
  return {
    node,
    adjacentEdges,
    sourceRefs,
    lines: snapshot.source_view.lines.filter((line) => sourceRefs.includes(line.source_ref)),
    locators: snapshot.source_locators.filter((locator) => sourceRefs.includes(locator.source_ref)),
  };
}

export function ExplainSourcePanel({ snapshot, onClose }: { snapshot: ExplainSnapshot; onClose(): void }): JSX.Element {
  const { text } = useLocale();
  const { nodeId, sourceRef, selectSourceRef } = useSelection();
  const closeButton = useRef<HTMLButtonElement>(null);
  const focus = useMemo(() => explainSourceFocus(snapshot, nodeId), [nodeId, snapshot]);
  const allRefs = useMemo(() => explainSourceRefs(snapshot), [snapshot]);
  const version = snapshot.version.selected_pack?.upstream_release ?? snapshot.version.detected_release ?? text("Unknown", "未知");
  const fragmentCount = new Set(snapshot.graph_view.nodes.map((node) => node.fragment_id).filter(Boolean)).size;
  const defaultLines = snapshot.source_view.lines.slice(0, 8);
  const displayedLines = focus.node === null ? defaultLines : focus.lines;
  const focusedLocator = sourceRef === null ? null : snapshot.source_locators.find((locator) => locator.source_ref === sourceRef) ?? null;

  useEffect(() => { closeButton.current?.focus({ preventScroll: true }); }, []);

  return <section className="explain-source-panel" aria-labelledby="explain-source-title" data-source-kind={snapshot.source_view.source_kind}>
    <header>
      <div><p className="eyebrow">{text("EXPLAIN COSTS", "EXPLAIN COSTS")}</p><h2 id="explain-source-title">{text("Source & provenance", "来源与溯源")}</h2></div>
      <button ref={closeButton} className="graph-tool" onClick={onClose} type="button">{text("Close source", "关闭来源")}</button>
    </header>
    <p className="explain-source-boundary">{text("Only backend-provided safe structural source is shown. SQL, database, table, column, and runtime values are not reconstructed here.", "这里只显示后端提供的安全结构来源；不会在此重建 SQL、数据库、表、列或运行时值。")}</p>
    <dl className="explain-source-meta">
      <div><dt>{text("Version", "版本")}</dt><dd>{version}</dd></div>
      <div><dt>{text("Structure", "结构")}</dt><dd>{fragmentCount} {text("fragments", "个片段")} · {snapshot.graph_view.nodes.length} {text("nodes", "个节点")} · {snapshot.graph_view.edges.length} {text("relations", "条关系")}</dd></div>
      <div><dt>{text("View", "视图")}</dt><dd>{snapshot.source_view.display_state}</dd></div>
    </dl>
    <section className="explain-version-sources" aria-label={text("Version source", "版本来源")}>
      <p className="eyebrow">{text("Version source", "版本来源")}</p>
      {snapshot.version.source_refs.length ? <div>{snapshot.version.source_refs.map((reference) => <button className={reference === sourceRef ? "focused" : ""} key={reference} onClick={() => selectSourceRef(reference)} type="button">{reference}</button>)}</div> : <p className="muted">{text("No version source reference was supplied.", "未提供版本来源引用。")}</p>}
    </section>
    <section className="explain-source-selection" aria-label={text("Selected plan evidence", "所选计划证据")}>
      <p className="eyebrow">{text("Selected plan evidence", "所选计划证据")}</p>
      {focus.node ? <><strong>{focus.node.display_name ?? focus.node.plan_node_type ?? focus.node.node_id}</strong><p>{[focus.node.plan_node_id ? `#${focus.node.plan_node_id}` : null, focus.node.fragment_id ? `${text("Fragment", "片段")} ${focus.node.fragment_id.replace(/^fragment-/, "F")}` : null].filter(Boolean).join(" · ")}</p></> : <p>{text("No plan node is selected. This concise list remains safe; select a visible plan node to narrow source and adjacent relations.", "尚未选择计划节点。以下简洁列表仍然安全；选择可见计划节点后可收窄来源与相邻关系。")}</p>}
      {focus.node && focus.adjacentEdges.length ? <ul className="explain-source-adjacent">{focus.adjacentEdges.map((edge) => <li key={edge.edge_id}><code>{edge.from} → {edge.to}</code><span>{edge.relation} · {edge.source}</span>{edge.evidence_refs.map((reference) => <button key={reference} onClick={() => selectSourceRef(reference)} type="button">{reference}</button>)}</li>)}</ul> : null}
    </section>
    <section className="explain-fact-source" aria-label={text("Fact source", "事实来源")}>
      <p className="eyebrow">{text("Fact source", "事实来源")}</p>
      {sourceRef === null ? <p className="muted">{text("Select a source reference to see where the fact comes from.", "选择来源引用，查看该事实出自计划文本的哪一行。")}</p> : <>
        <code>{sourceRef}</code>
        <p>{focusedLocator ? locatorLabel(focusedLocator, text) : text("Typed locator unavailable", "类型化定位器不可用")}</p>
      </>}
      <span className="explain-restricted-attribute">restricted_attribute_text</span>
    </section>
    <div className="explain-source-content">
      {displayedLines.length ? <ol className="explain-source-lines">{displayedLines.map((line, index) => <li className={line.source_ref === sourceRef ? "focused" : ""} key={`${line.source_ref}:${index}`}><button data-source-ref={line.source_ref} onClick={() => selectSourceRef(line.source_ref)} type="button"><code>{line.line_no}</code><span>{line.text}</span></button></li>)}</ol> : <p className="muted">{focus.node ? text("No safe source lines match this selected plan node.", "没有与所选计划节点匹配的安全来源行。") : text("No safe source lines were supplied.", "未提供安全来源行。")}</p>}
    </div>
    <section className="explain-source-refs" aria-label={text("Focused Explain source references", "聚焦 Explain 来源引用")}>
      <p className="eyebrow">{focus.node ? text("Focused source references", "聚焦来源引用") : text("Safe source references", "安全来源引用")}</p>
      {focus.sourceRefs.length ? <div>{focus.sourceRefs.map((reference) => <button className={reference === sourceRef ? "focused" : ""} key={reference} onClick={() => selectSourceRef(reference)} type="button">{reference}</button>)}</div> : <p className="muted">{text("No focused source references were supplied.", "未提供聚焦来源引用。")}</p>}
    </section>
    <details className="explain-source-locators">
      <summary>{text("All source references", "全部来源引用")} ({snapshot.source_locators.length})</summary>
      {snapshot.source_locators.length ? <ul>{snapshot.source_locators.map((locator, index) => <li key={`${locator.kind}:${locator.source_ref}:${index}`}><button className={locator.source_ref === sourceRef ? "focused" : ""} onClick={() => selectSourceRef(locator.source_ref)} type="button"><code>{locator.source_ref}</code><span>{locatorLabel(locator, text)}</span></button></li>)}</ul> : <p className="muted">{text("No source references were supplied.", "未提供来源引用。")}</p>}
    </details>
    <details className="explain-source-all-refs">
      <summary>{text("All safe source references", "全部安全来源引用")} ({allRefs.length})</summary>
      <div>{allRefs.map((reference) => <button className={reference === sourceRef ? "focused" : ""} key={reference} onClick={() => selectSourceRef(reference)} type="button">{reference}</button>)}</div>
    </details>
    {sourceRef !== null && !snapshot.source_view.lines.some((line) => line.source_ref === sourceRef) ? <p className="explain-source-unavailable" role="status">{text("The selected reference has no matching plan text line.", "所选引用没有对应的计划文本行。")}</p> : null}
    {snapshot.source_view.limitations.length ? <section className="explain-source-limitations"><p className="eyebrow">{text("Source limitations", "来源限制")}</p><ul>{snapshot.source_view.limitations.map((limitation) => <li key={limitation}>{limitation}</li>)}</ul></section> : null}
  </section>;
}

function locatorLabel(locator: ExplainSourceLocator, text: (english: string, chinese: string) => string): string {
  if (locator.kind === "result") return `${text("Result", "结果")} · ${locator.row_count} ${text("rows", "行")}`;
  if (locator.kind === "row_line") return `${text("Row", "行")} ${locator.row_no} · ${text("line", "行内行")} ${locator.row_line_no}`;
  return `${text("Connection metadata", "连接元数据")} · ${locator.availability}`;
}
