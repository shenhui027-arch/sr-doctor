import type { ExplainSnapshot } from "../../../../contracts/generated/doctor-contracts";
import { useEffect, useState, type JSX } from "react";
import { useLocale } from "../../i18n/locale";
import { ExecutionGraph } from "../execution/ExecutionGraph";
import { SelectionProvider } from "../selection/selection-store";
import { ExplainSourcePanel } from "./ExplainSourcePanel";

export type ExplainGraphPanelProps = {
  snapshot: ExplainSnapshot;
  initialSourceOpen?: boolean;
  onClose(): void;
};

/**
 * The Explain plan uses the same execution graph as a Profile, without runtime
 * metrics: cards carry the backend-selected plan facts instead. A snapshot-keyed
 * selection scope keeps Explain selection out of Profile views.
 */
export function ExplainGraphPanel({ snapshot, initialSourceOpen = false, onClose }: ExplainGraphPanelProps): JSX.Element {
  const { text } = useLocale();
  const hasGraph = snapshot.graph_view.state !== "unavailable";
  const [sourceOpen, setSourceOpen] = useState(initialSourceOpen || !hasGraph);
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);
  return <SelectionProvider key={snapshot.snapshot_id}>
    <aside className="explain-plan-overlay" role="dialog" aria-label={text("Explain plan", "Explain 计划")}>
      <header className="explain-plan-overlay-header">
        <div><strong>{text("Readonly plan", "只读计划")} · {snapshot.explain_level.toUpperCase()}</strong><span>{text("Estimates only; nothing was executed.", "仅为估算，未执行任何查询。")}</span></div>
        <div className="execution-graph-tools">
          {hasGraph ? <button className="graph-tool" aria-pressed={sourceOpen} onClick={() => setSourceOpen((open) => !open)} type="button">{sourceOpen ? text("Hide plan text", "隐藏计划文本") : text("Plan text", "计划文本")}</button> : null}
          <button className="graph-tool" onClick={onClose} type="button">{text("Close", "关闭")}</button>
        </div>
      </header>
      <div className={`explain-plan-overlay-body${sourceOpen && hasGraph ? " with-source" : ""}`}>
        {hasGraph ? <ExecutionGraph factSets={snapshot.plan_node_fact_sets} graph={snapshot.graph_view} graphKey={snapshot.snapshot_id} /> : null}
        {sourceOpen ? <ExplainSourcePanel onClose={() => (hasGraph ? setSourceOpen(false) : onClose())} snapshot={snapshot} /> : null}
      </div>
    </aside>
  </SelectionProvider>;
}
