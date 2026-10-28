import {
  BaseEdge,
  EdgeLabelRenderer,
  Handle,
  MarkerType,
  Position,
  ReactFlow,
  ReactFlowProvider,
  ViewportPortal,
  useReactFlow,
  useStore,
  type Edge,
  type EdgeProps,
  type Node,
  type NodeProps,
} from "@xyflow/react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type JSX, type ReactNode } from "react";
import type { ExecutionSummary, ExplainNodeFactSet, GraphNode, GraphSqlFact, GraphView, PlanNodeSummary, SourceView } from "../../../../contracts/generated/doctor-contracts";
import { localizedEnum } from "../../i18n/enum-label";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { useSelection } from "../selection/selection-store";
import { explainCardFacts, explainFactLabel, explainFactValue } from "./explain-facts";
import { dataFlowLayout, type DataFlowLayout } from "./data-flow-layout";
import { startLayoutTask } from "./graph-layout-client";
import type { LayoutInput, LayoutResult } from "./graph-layout";
import { NodeSqlLocation } from "./NodeSqlLocation";
import "./execution-graph.css";

/** Card geometry is shared by the CSS and the ELK input. */
export const GRAPH_CARD = { width: 248, height: 104 } as const;
/** Doctor runtime cards (design board 02): operator, Plan id, one fact line and the time share. */
export const GRAPH_CARD_COMPACT = { width: 248, height: 84 } as const;
/** Arrowhead drawn at the end of a routed edge (design board 02). */
const ARROW = { width: 10, length: 7 } as const;
const PORT = 6;
/**
 * Declared port geometry (relative to the card) so edges render without waiting
 * for DOM measurement, which may lag a frame or never run in a hidden view.
 */
const PLAN_CARD_HANDLES = [
  { type: "source", position: Position.Top, x: (GRAPH_CARD.width - PORT) / 2, y: -PORT / 2, width: PORT, height: PORT },
  { type: "target", position: Position.Bottom, x: (GRAPH_CARD.width - PORT) / 2, y: GRAPH_CARD.height - PORT / 2, width: PORT, height: PORT },
] as const;
const FIT_ALL_NODE_LIMIT = 12;

type CardData = Record<string, unknown> & {
  node: GraphNode;
  summary: PlanNodeSummary | null;
  facts: { label: string; value: string }[];
  selected: boolean;
  /** The plan node of the finding open in the inspector. */
  focused: boolean;
  findingCount: number;
  /** Doctor card: share bar scaled to the hottest node (layout only; the text is the backend share). */
  compact: boolean;
  barPermyriad: number;
  unconnected: boolean;
};
type CardNode = Node<CardData, "plan">;
type RouteEdge = Edge<{ route?: { x: number; y: number }[]; arrow?: string; label?: string }, "route">;

/** One rendered relation in data-flow direction (upstream → downstream). */
export type FlowEdge = { id: string; source: string; target: string; derived: boolean; partial: boolean; role: "probe" | "build" | null };

/**
 * Converts typed graph relations to data-flow direction. An edge the backend
 * marks as restating another (`restates_edge_id`: a `fragment_sink` repeating
 * a `plan_child` exchange edge in the opposite direction) is skipped. Nothing
 * is invented or deduplicated here; only DTO edges are drawn.
 */
export function flowEdges(graph: GraphView): FlowEdge[] {
  const nodeIds = new Set(graph.nodes.map((node) => node.node_id));
  const edges: FlowEdge[] = [];
  for (const edge of graph.edges) {
    if (edge.restates_edge_id !== null) continue;
    const [source, target] = edge.direction_semantics === "parent_to_child" ? [edge.to, edge.from] : [edge.from, edge.to];
    if (!nodeIds.has(source) || !nodeIds.has(target) || source === target) continue;
    const role = edge.input_role === "probe" || edge.input_role === "build" ? edge.input_role : null;
    edges.push({ id: edge.edge_id, source, target, derived: edge.source === "verified_derived", partial: edge.quality === "partial", role });
  }
  return edges;
}

/** Fragment frames are drawn only when the DTO places nodes in two or more fragments. */
export function fragmentGroups(graph: GraphView, summaries: ReadonlyMap<string, PlanNodeSummary>): { id: string; fragment: string; nodeIds: string[] }[] {
  const members = new Map<string, string[]>();
  for (const node of graph.nodes) {
    const fragment = summaries.get(node.node_id)?.fragment_ref ?? node.fragment_id;
    if (fragment === null || fragment === undefined) continue;
    members.set(fragment, [...(members.get(fragment) ?? []), node.node_id]);
  }
  if (members.size < 2) return [];
  const nodeIds = new Set(graph.nodes.map((node) => node.node_id));
  return [...members.entries()].map(([fragment, ids]) => {
    let id = `fragment:${fragment}`;
    while (nodeIds.has(id)) id = `_${id}`;
    return { id, fragment, nodeIds: ids };
  });
}

/** The hottest exact node, by backend basis points; ties keep DTO order. */
export function hottestNodeId(summary: ExecutionSummary | null | undefined): string | null {
  let best: PlanNodeSummary | null = null;
  for (const node of summary?.state === "available" ? summary.nodes : []) {
    if (node.time_share_basis_points === null) continue;
    if (!best || node.time_share_basis_points > (best.time_share_basis_points ?? -1)) best = node;
  }
  return best?.node_id ?? null;
}

export function primarySqlFact(node: GraphNode): GraphSqlFact | null {
  const facts = node.sql_facts ?? [];
  return facts.find((fact) => fact.kind === "table") ?? facts.find((fact) => fact.kind === "join_condition") ?? facts.find((fact) => fact.kind === "predicate") ?? null;
}

export type OperatorCategory = "scan" | "exchange" | "join" | "agg" | "other";

/** Display colour of a plan node, from its DTO plan node type only. */
export function operatorCategory(type: string | null | undefined): OperatorCategory {
  const value = (type ?? "").toUpperCase();
  if (value.includes("SCAN")) return "scan";
  if (value.includes("EXCHANGE")) return "exchange";
  if (value.includes("JOIN")) return "join";
  if (value.includes("AGGREGAT")) return "agg";
  return "other";
}

/** The label on a data-flow edge: the upstream node's backend output rows, with the join input side. */
export function flowEdgeLabel(edge: FlowEdge, upstream: PlanNodeSummary | null | undefined, text: LocalizedText): string | undefined {
  const rows = upstream?.output_rows.display_text;
  const side = edge.role === "probe" ? text("Probe", "探测") : edge.role === "build" ? text("Build", "构建") : null;
  if (!rows) return side ?? undefined;
  const value = `${rows} ${text("rows", "行")}`;
  return side ? `${side} · ${value}` : value;
}

export function hotspotLabel(summary: PlanNodeSummary | null, text: LocalizedText): string | null {
  if (summary?.hotspot === "most_consuming") return text("Top cost", "最耗时");
  if (summary?.hotspot === "second_most_consuming") return text("High cost", "次耗时");
  // The counters' print precision straddles a 15% / 30% threshold: the tier is undecidable.
  if (summary?.hotspot === "boundary") return text("Near threshold", "临界");
  return null;
}

const nodeTypes = { plan: PlanCard };
const edgeTypes = { route: RoutedEdge };

export type ExecutionGraphProps = {
  graph: GraphView;
  /** Runtime Profile: per-node ExplainAnalyzer summary. Absent for Explain. */
  summary?: ExecutionSummary | null;
  /** Explain: backend-selected plan facts per node. */
  factSets?: readonly ExplainNodeFactSet[];
  /** Workspace identity; a new key resets the viewport. */
  graphKey: string;
  /** Doctor: each finding's located plan node (backend subject, resolved by identity). */
  findingTargets?: readonly FindingTarget[];
  /** Doctor: the finding the inspector shows when none is selected. */
  defaultFindingId?: string | null;
  /** Runtime Profile: the source view whose lines name the inspector's source references. */
  sourceView?: SourceView | null;
  /** Doctor: a third view tab after 数据流 / 原始结构 (the official-reading entry). */
  viewExtra?: ReactNode;
};

type GraphGeometry = { positions: Record<string, { x: number; y: number }>; groups: { id: string; x: number; y: number; width: number; height: number }[]; routes: Record<string, { x: number; y: number }[]> };

export type FindingTarget = { findingId: string; subjectRef: string | null; nodeId: string; title: string; severity: string };

/** Below this fit zoom the cards are unreadable; the first view opens on the focus node instead. */
const READABLE_ZOOM = 0.6;

export function ExecutionGraph(props: ExecutionGraphProps): JSX.Element {
  return <ReactFlowProvider key={props.graphKey}><GraphCanvas {...props} /></ReactFlowProvider>;
}

function GraphCanvas({ graph, summary, factSets, findingTargets = [], defaultFindingId = null, sourceView = null, viewExtra = null }: ExecutionGraphProps): JSX.Element {
  const { text } = useLocale();
  const { nodeId, findingId, selectNode, selectFinding, clearSelection, selectSourceRef } = useSelection();
  const { fitView, getZoom, setCenter, setViewport, zoomIn, zoomOut } = useReactFlow<CardNode, RouteEdge>();
  const canvasWidth = useStore((state) => state.width);
  const canvasHeight = useStore((state) => state.height);
  const [expanded, setExpanded] = useState(false);
  // Doctor (runtime summary) draws data flow top-down by default; Explain keeps the plan tree.
  const doctorView = summary !== undefined && summary !== null && !(factSets?.length);
  const [view, setView] = useState<"flow" | "plan">("flow");
  const flowView = doctorView && view === "flow";
  const card = doctorView ? GRAPH_CARD_COMPACT : GRAPH_CARD;
  const [geometry, setGeometry] = useState<{ input: LayoutInput; result?: LayoutResult; error?: string } | null>(null);
  const sectionRef = useRef<HTMLElement>(null);
  const centeredFor = useRef<string | null>(null);

  const summaries = useMemo(() => new Map((summary?.state === "available" ? summary.nodes : []).map((node) => [node.node_id, node] as const)), [summary]);
  const factsByNode = useMemo(() => new Map((factSets ?? []).map((set) => [set.node_ref, set] as const)), [factSets]);
  const sources = useMemo(() => sourceLabels(sourceView), [sourceView]);
  const edges = useMemo(() => flowEdges(graph), [graph]);
  const groups = useMemo(() => fragmentGroups(graph, summaries), [graph, summaries]);
  const hottest = useMemo(() => hottestNodeId(summary), [summary]);
  const activeFindingId = findingId ?? defaultFindingId;
  const focusNodeId = findingTargets.find((target) => target.findingId === activeFindingId)?.nodeId ?? null;
  const findingCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const target of findingTargets) counts.set(target.nodeId, (counts.get(target.nodeId) ?? 0) + 1);
    return counts;
  }, [findingTargets]);
  // A single in-tree draws as the designed data-flow tree; anything else falls back to ELK.
  const tree = useMemo<DataFlowLayout | null>(() => flowView && groups.length === 0 ? dataFlowLayout({
    nodes: graph.nodes.map((node) => ({ id: node.node_id, planNodeId: node.plan_node_id })),
    edges, unconnected: graph.unconnected_node_ids, card,
  }) : null, [card, edges, flowView, graph.nodes, graph.unconnected_node_ids, groups.length]);
  const layoutInput = useMemo<LayoutInput>(() => ({
    nodes: graph.nodes.map((node) => ({ id: node.node_id })),
    // Plan view runs result-first (downstream above upstream); data-flow view runs sources first. Arrows keep data-flow direction.
    edges: edges.map((edge) => ({ id: edge.id, source: edge.source, target: edge.target, reverseForLayout: !flowView })),
    groups: groups.map((group) => ({ id: group.id, nodeIds: group.nodeIds })),
    nodeWidth: card.width,
    nodeHeight: card.height,
    density: graph.nodes.length > 30 ? "dense" : "standard",
    edgeLabels: flowView,
  }), [card, edges, flowView, graph.nodes, groups]);

  useEffect(() => {
    if (tree) return;
    setGeometry({ input: layoutInput });
    return startLayoutTask(layoutInput, (result) => setGeometry({ input: layoutInput, result }), (error) => setGeometry({ input: layoutInput, error }));
  }, [layoutInput, tree]);
  const layout: GraphGeometry | undefined = tree ? { positions: tree.positions, routes: tree.routes, groups: [] } : geometry?.input === layoutInput ? geometry.result : undefined;
  const layoutError = tree ? undefined : geometry?.input === layoutInput ? geometry.error : undefined;
  const maxShare = useMemo(() => Math.max(0, ...[...summaries.values()].map((node) => node.time_share_basis_points ?? 0)), [summaries]);
  const unconnected = useMemo(() => new Set(graph.unconnected_node_ids), [graph.unconnected_node_ids]);

  const nodes = useMemo<CardNode[]>(() => layout ? graph.nodes.map((node) => ({
    id: node.node_id,
    type: "plan",
    position: layout.positions[node.node_id] ?? { x: 0, y: 0 },
    width: card.width,
    height: card.height,
    handles: (flowView ? flowHandles(card) : planHandles(card)).map((handle) => ({ ...handle })),
    draggable: false,
    selectable: true,
    data: {
      node, summary: summaries.get(node.node_id) ?? null, facts: explainCardFacts(factsByNode.get(node.node_id), text), selected: node.node_id === nodeId, focused: node.node_id === focusNodeId, findingCount: findingCounts.get(node.node_id) ?? 0,
      compact: doctorView, barPermyriad: maxShare > 0 ? Math.round((summaries.get(node.node_id)?.time_share_basis_points ?? 0) * 10000 / maxShare) : 0, unconnected: unconnected.has(node.node_id),
    },
  })) : [], [card, doctorView, factsByNode, findingCounts, flowView, focusNodeId, graph.nodes, layout, maxShare, nodeId, summaries, text, unconnected]);
  const flow = useMemo<RouteEdge[]>(() => layout ? edges.map((edge) => {
    // Explicit: solid grey; verified derived: dashed accent; partial: dotted (topology restyle).
    const color = edge.partial ? "var(--dashed)" : edge.derived ? "var(--accent)" : doctorView ? "var(--graph-edge)" : "var(--edge)";
    return {
      id: edge.id,
      source: edge.source,
      target: edge.target,
      type: "route",
      data: { route: layout.routes[edge.id], arrow: doctorView ? color : undefined, label: flowView ? flowEdgeLabel(edge, summaries.get(edge.source), text) : undefined },
      markerEnd: doctorView ? undefined : { type: MarkerType.ArrowClosed, color, width: 14, height: 14 },
      style: { stroke: color, strokeWidth: doctorView ? 1.5 : 1.6, strokeDasharray: edge.partial ? "2 4" : edge.derived ? (doctorView ? "6 4" : "5 4") : undefined },
    };
  }) : [], [doctorView, edges, flowView, layout, summaries, text]);

  const centerOn = useCallback((id: string, zoom?: number) => {
    const point = layout?.positions[id];
    if (point) void setCenter(point.x + card.width / 2, point.y + card.height / 2, { zoom: zoom ?? 0.9, duration: 200 });
  }, [card, layout, setCenter]);
  // A data-flow tree that fits is drawn at 1:1, its result column on the canvas centre and its sources 20 px below the top.
  const placeTree = useCallback((duration = 0): boolean => {
    if (!tree || canvasWidth <= 0 || tree.bounds.width + 40 > canvasWidth || tree.bounds.height + 40 > Math.max(canvasHeight, 1)) return false;
    void setViewport({ x: Math.round(canvasWidth / 2 - tree.rootCenterX), y: 20 - tree.bounds.y, zoom: 1 }, { duration });
    return true;
  }, [canvasHeight, canvasWidth, setViewport, tree]);
  const fitAll = useCallback(() => { if (!placeTree(200)) void fitView({ padding: 0.12, duration: 200 }); }, [fitView, placeTree]);
  // Like centerOn, but never leaves empty canvas above the plan's top row: a hot source sits near the top, not mid-canvas.
  const focusReadable = useCallback((id: string, zoom: number) => {
    const point = layout?.positions[id];
    if (!point || canvasWidth <= 0 || canvasHeight <= 0) { centerOn(id, zoom); return; }
    const top = Math.min(...Object.values(layout.positions).map((position) => position.y));
    const centeredY = canvasHeight / 2 - (point.y + card.height / 2) * zoom;
    void setViewport({ x: canvasWidth / 2 - (point.x + card.width / 2) * zoom, y: Math.min(centeredY, 20 - top * zoom), zoom }, { duration: 0 });
  }, [canvasHeight, canvasWidth, card, centerOn, layout, setViewport]);
  // The first-view effect below must not re-run on a canvas height change, so it reads the latest focus through a ref.
  const focusReadableRef = useRef(focusReadable);
  focusReadableRef.current = focusReadable;

  // First view: the whole plan when it stays readable; otherwise readable on the
  // open finding's node (or the hottest node), so the slow node is visible at once.
  const initialFocus = useRef<string | null>(null);
  initialFocus.current = focusNodeId ?? hottest ?? graph.root_node_ids[0] ?? null;
  useEffect(() => {
    if (!layout) return;
    // A data-flow tree waits for the measured canvas so it is placed once, at 1:1, without an interim fit.
    if (tree && canvasWidth <= 0) return;
    const timer = window.setTimeout(() => {
      const target = initialFocus.current;
      if (placeTree()) return;
      if (graph.nodes.length <= FIT_ALL_NODE_LIMIT || !target) {
        void fitView({ padding: 0.15, maxZoom: 1, duration: 0 }).then(() => {
          if (target && getZoom() < READABLE_ZOOM) focusReadableRef.current(target, 0.85);
        });
      } else focusReadableRef.current(target, 0.85);
    }, 0);
    return () => window.clearTimeout(timer);
  }, [canvasWidth, centerOn, fitView, getZoom, graph.nodes.length, layout, placeTree, tree]);

  // Choosing another finding brings its plan node into view.
  const focusedFor = useRef<string | null>(null);
  useEffect(() => {
    if (!layout || !focusNodeId) return;
    if (focusedFor.current === null) { focusedFor.current = focusNodeId; return; }
    if (focusedFor.current === focusNodeId) return;
    focusedFor.current = focusNodeId;
    if (!nodeId && layout.positions[focusNodeId]) centerOn(focusNodeId, Math.max(getZoom(), 0.85));
  }, [centerOn, focusNodeId, getZoom, layout, nodeId]);

  // External selection (node table, recommendation "locate") brings the node into view.
  useEffect(() => {
    if (!layout || !nodeId || centeredFor.current === nodeId) return;
    centeredFor.current = nodeId;
    if (layout.positions[nodeId]) centerOn(nodeId);
  }, [centerOn, layout, nodeId]);

  useLayoutEffect(() => {
    if (!expanded) return;
    const overflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    const onKey = (event: KeyboardEvent) => { if (event.key === "Escape" && !event.defaultPrevented) setExpanded(false); };
    window.addEventListener("keydown", onKey);
    return () => { document.body.style.overflow = overflow; window.removeEventListener("keydown", onKey); };
  }, [expanded]);
  // Entering or leaving full screen refits; the first render is placed by the effect above.
  const previousExpanded = useRef(expanded);
  useEffect(() => {
    // Only an actual toggle refits (StrictMode re-runs mount effects).
    if (previousExpanded.current === expanded) return;
    previousExpanded.current = expanded;
    if (layout) window.setTimeout(() => { if (!placeTree()) void fitView({ padding: 0.12, duration: 0 }); }, 0);
  }, [expanded]);

  const selected = graph.nodes.find((node) => node.node_id === nodeId) ?? null;
  const hotspotCount = [...summaries.values()].filter((node) => node.hotspot === "most_consuming" || node.hotspot === "second_most_consuming").length;
  const fragmentCount = groups.length || new Set(graph.nodes.map((node) => node.fragment_id).filter(Boolean)).size;
  const unresolved = graph.unresolved_relations.length;

  const counts = `${graph.nodes.length} ${text("plan nodes", "个计划节点")}${!doctorView && fragmentCount ? ` · ${fragmentCount} Fragment` : ""}${summary?.state === "available" ? ` · ${hotspotCount} ${text("high-cost", "个高耗时")}` : ""}`;
  return <section ref={sectionRef} className={`panel execution-graph${doctorView ? " is-doctor" : ""}${expanded ? " is-expanded" : ""}`} aria-label={doctorView ? text("Execution structure", "执行结构") : text("Execution graph", "执行图")}>
    <header className="execution-graph-header">
      <h2>{doctorView ? text("Execution structure", "执行结构") : text("Execution graph", "执行图")}</h2>
      {doctorView ? <div aria-label={text("View", "视图")} className="graph-view-switch" role="group">
        <button aria-pressed={view === "flow"} onClick={() => setView("flow")} type="button">{text("Data flow", "数据流")}</button>
        <button aria-pressed={view === "plan"} onClick={() => setView("plan")} type="button">{text("Plan structure", "原始结构")}</button>
        {viewExtra}
      </div> : null}
      {/* The count names the high-cost nodes; clicking it goes to the hottest one. */}
      {hottest ? <button className="graph-counts" onClick={() => { selectNode(hottest); centerOn(hottest); }} title={text("Show the hottest node", "定位最耗时节点")} type="button">{counts}</button> : <span className="graph-counts">{counts}</span>}
      <div className="execution-graph-tools" role="toolbar" aria-label={text("Graph controls", "图控制")}>
        <button className="graph-tool icon" onClick={() => void zoomOut({ duration: 150 })} type="button" aria-label={text("Zoom out", "缩小")}><svg aria-hidden="true" fill="none" height="14" stroke="currentColor" strokeWidth="2" viewBox="0 0 24 24" width="14"><path d="M5 12h14" /></svg></button>
        <button className="graph-tool icon" onClick={() => void zoomIn({ duration: 150 })} type="button" aria-label={text("Zoom in", "放大")}><svg aria-hidden="true" fill="none" height="14" stroke="currentColor" strokeWidth="2" viewBox="0 0 24 24" width="14"><path d="M12 5v14M5 12h14" /></svg></button>
        <button className="graph-tool" onClick={fitAll} type="button">{text("Fit all", "适配全图")}</button>
        <button className="graph-tool icon" aria-label={expanded ? text("Exit full screen", "退出全屏") : text("Full screen", "全屏")} aria-pressed={expanded} onClick={() => setExpanded((value) => !value)} type="button"><svg aria-hidden="true" fill="none" height="14" stroke="currentColor" strokeWidth="2" viewBox="0 0 24 24" width="14"><path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5" /></svg></button>
      </div>
    </header>
    <div className={`execution-graph-canvas${selected ? " has-inspector" : ""}`} data-view={flowView ? "flow" : "plan"} data-layout-state={layout ? "ready" : layoutError ? "failed" : "loading"}>
      {graph.nodes.length === 0 ? <p className="execution-graph-status" role="status">{text("No execution structure could be read from this Profile; the Profile notice above and the evidence gaps say why.", "无法从此 Profile 读取执行结构；原因见上方 Profile 提示与证据缺口。")}</p>
        : !layout ? <p className="execution-graph-status" role="status">{layoutError ? text("Layout unavailable; the node table below still lists every plan node.", "布局不可用；下方节点表仍列出全部计划节点。") : text("Arranging the plan…", "正在排列计划…")}</p> : null}
      <ReactFlow<CardNode, RouteEdge>
        edgeTypes={edgeTypes}
        edges={flow}
        maxZoom={1.6}
        minZoom={0.08}
        nodeTypes={nodeTypes}
        nodes={nodes}
        nodesConnectable={false}
        nodesDraggable={false}
        onNodeClick={(_event, node) => selectNode(node.id)}
        onPaneClick={() => clearSelection()}
        panOnScroll={false}
        preventScrolling={false}
        proOptions={{ hideAttribution: false }}
        zoomActivationKeyCode="Control"
        zoomOnScroll={false}
      >
        {layout ? <ViewportPortal>{layout.groups.map((frame) => {
          const group = groups.find((item) => item.id === frame.id);
          return <div aria-hidden="true" className="fragment-frame" key={frame.id} style={{ left: frame.x, top: frame.y, width: frame.width, height: frame.height }}><span>Fragment {group?.fragment}</span></div>;
        })}</ViewportPortal> : null}
      </ReactFlow>
      {selected ? <NodeInspector factSet={factsByNode.get(selected.node_id)} findings={findingTargets.filter((target) => target.nodeId === selected.node_id)} node={selected} onClose={() => clearSelection()} onFinding={(target) => selectFinding(target.findingId, target.subjectRef)} onSource={selectSourceRef} sources={sources} summary={summaries.get(selected.node_id) ?? null} /> : null}
    </div>
    <footer className="execution-graph-legend" title={text("Drag to pan · Ctrl + wheel to zoom · click a node for details", "拖动平移 · Ctrl+滚轮缩放 · 点击节点看详情")}>
      {doctorView ? <span className="legend-categories">
        <i className="legend-chip cat-scan">{text("Scan", "扫描")}</i>
        <i className="legend-chip cat-exchange">{text("Exchange", "交换")}</i>
        <i className="legend-chip cat-join">{text("Join", "连接")}</i>
        <i className="legend-chip cat-agg">{text("Aggregate", "聚合")}</i>
        <i className="legend-chip cat-other">{text("Other", "其他")}</i>
        {findingTargets.length ? <i className="legend-chip has-findings">{text("Has findings", "有发现项")}</i> : null}
      </span> : null}
      <span><i className="legend-line" />{doctorView ? text("Explicit relation", "显式关系") : text("Data flow", "数据流向")}</span>
      <span><i className="legend-line derived" />{text("Verified derived", "已验证推导")}</span>
      {doctorView ? <span><i className="legend-line partial" />{text("Partial", "部分")}</span> : null}
      {doctorView && unconnected.size ? <span><i className="legend-box" />{text(`${unconnected.size} relation(s) unproven, not drawn`, `${unconnected.size} 处关系无法证实，未绘制`)}</span> : null}
      {unresolved && graph.state !== "unavailable" ? <span className="legend-warning">{unresolved} {text("structural check(s) failed against the official topology; graph is partial", "处与官方拓扑不变量不符，图标为部分")}</span> : null}
      {summary?.state === "available" ? <span className="legend-heat"><i className="legend-swatch most" />{text("Top cost > 30%", "最耗时 > 30%")}<i className="legend-swatch second" />{text("High cost > 15%", "次耗时 > 15%")}</span> : null}
    </footer>
  </section>;
}

/** Card ports: inputs arrive on top, the output leaves at the bottom (data-flow view). */
function flowHandles(card: { width: number; height: number }) {
  return [
    { type: "source" as const, position: Position.Bottom, x: (card.width - PORT) / 2, y: card.height - PORT / 2, width: PORT, height: PORT },
    { type: "target" as const, position: Position.Top, x: (card.width - PORT) / 2, y: -PORT / 2, width: PORT, height: PORT },
  ];
}

function planHandles(card: { width: number; height: number }) {
  return card.width === GRAPH_CARD.width ? PLAN_CARD_HANDLES : [
    { type: "source" as const, position: Position.Top, x: (card.width - PORT) / 2, y: -PORT / 2, width: PORT, height: PORT },
    { type: "target" as const, position: Position.Bottom, x: (card.width - PORT) / 2, y: card.height - PORT / 2, width: PORT, height: PORT },
  ];
}

/** One line under the operator: the table it reads or its main plan fact, then its output rows. */
function compactCardLine(node: GraphNode, summary: PlanNodeSummary | null, unconnected: boolean, text: LocalizedText): string {
  if (unconnected) return text("Up/downstream unproven · not connected", "上下游未证实 · 不连线");
  const fact = primarySqlFact(node) ?? (node.sql_facts ?? []).find((candidate) => candidate.kind === "fields") ?? null;
  const rows = summary?.output_rows.display_text ?? null;
  if (fact?.kind === "table") return `${text("Table", "表")} ${fact.value.split(".").pop()}${rows ? ` · ${rows} ${text("rows out", "行输出")}` : ""}`;
  if (fact) return `${fact.value}${rows ? ` · ${rows} ${text("rows", "行")}` : ""}`;
  if (node.plan_node_type?.includes("SINK")) return `${text("Result output", "结果出口")}${rows ? ` · ${rows} ${text("rows", "行")}` : ""}`;
  return rows ? `${rows} ${text("rows out", "行输出")}` : node.fragment_id ? `Fragment ${node.fragment_id}` : " ";
}

function PlanCard({ data }: NodeProps<CardNode>): JSX.Element {
  const { text } = useLocale();
  const { node, summary, facts, selected, focused, findingCount } = data;
  if (data.compact) return <CompactPlanCard data={data} />;
  const title = node.display_name ?? node.plan_node_type ?? node.node_id;
  const fact = primarySqlFact(node);
  const hotspot = hotspotLabel(summary, text);
  const share = summary?.time_share_basis_points ?? null;
  return <article className={`plan-card${selected ? " selected" : ""}${focused ? " finding-focus" : ""}${summary?.hotspot === "most_consuming" ? " hot-most" : summary?.hotspot === "second_most_consuming" ? " hot-second" : ""}`} data-node-id={node.node_id} tabIndex={-1}>
    <Handle className="plan-port" position={Position.Bottom} type="target" />
    <header><strong title={title}>{title}</strong>{hotspot ? <em>{hotspot}</em> : null}{findingCount ? <i className="plan-card-findings" title={text(`${findingCount} finding(s) on this node`, `此节点有 ${findingCount} 条发现`)}>{findingCount}</i> : null}<span>#{node.plan_node_id ?? node.node_id}</span></header>
    <p className="plan-card-fact" title={fact?.value}>{fact ? fact.value : node.fragment_id ? `Fragment ${node.fragment_id}` : " "}</p>
    {summary ? <>
      <div className="plan-card-time"><span className="share-bar" aria-hidden="true"><span style={{ width: `${Math.min(100, (share ?? 0) / 100)}%` }} /></span><b>{summary.time_share_display ?? "—"}</b><span>{summary.total_time.display_text ?? text("— unavailable", "— 不可用")}</span></div>
      <p className="plan-card-rows">{text("Rows", "输出行")} {summary.output_rows.display_text ?? text("— unavailable", "— 不可用")}</p>
    </> : facts.length ? <dl className="plan-card-facts">{facts.slice(0, 2).map((item) => <div key={item.label}><dt>{item.label}</dt><dd>{item.value}</dd></div>)}</dl> : <p className="plan-card-rows muted">{text("No runtime metrics", "无运行时指标")}</p>}
    <Handle className="plan-port" position={Position.Top} type="source" />
  </article>;
}

function CompactPlanCard({ data }: { data: CardData }): JSX.Element {
  const { text } = useLocale();
  const { node, summary, selected, focused, findingCount, barPermyriad, unconnected } = data;
  const title = node.display_name ?? node.plan_node_type ?? node.node_id;
  const tone = unconnected || !summary ? "gone" : summary.hotspot === "most_consuming" ? "hot-most" : summary.hotspot === "second_most_consuming" ? "hot-second" : "cool";
  const hotspot = hotspotLabel(summary, text);
  const category = operatorCategory(node.plan_node_type ?? node.display_name);
  return <article aria-label={`${title} Plan ${node.plan_node_id ?? node.node_id}${summary?.time_share_display ? ` ${text("share of operator time", "占算子总耗时")} ${summary.time_share_display}` : ""}${findingCount ? ` · ${text(`${findingCount} finding(s)`, `${findingCount} 条发现`)}` : ""}`} className={`plan-card compact cat-${category} ${tone}${findingCount ? " has-findings" : ""}${selected ? " selected" : ""}${focused ? " finding-focus" : ""}`} data-node-id={node.node_id} tabIndex={-1} title={hotspot ?? undefined}>
    <Handle className="plan-port" position={Position.Top} type="target" />
    <header><strong>{title}</strong>{findingCount ? <em className="plan-card-finding-pill">{text(`${findingCount} finding${findingCount > 1 ? "s" : ""}`, `${findingCount} 条发现`)}</em> : null}<span>Plan {node.plan_node_id ?? node.node_id}</span></header>
    <p className="plan-card-line">{compactCardLine(node, summary, unconnected, text)}</p>
    <div className="plan-card-share"><span className="share-bar" aria-hidden="true"><span style={{ width: `${barPermyriad / 100}%` }} /></span><b>{summary?.time_share_display ?? text("No metrics", "无指标")}</b>{summary?.total_time.display_text ? <span>{summary.total_time.display_text}</span> : null}</div>
    <Handle className="plan-port" position={Position.Bottom} type="source" />
  </article>;
}

/** A polyline with its bends rounded (radius up to 12 px), as in the topology restyle. */
export function roundedPath(points: readonly { x: number; y: number }[], radius = 12): string {
  if (points.length < 3) return points.map((point, index) => `${index === 0 ? "M" : "L"} ${point.x} ${point.y}`).join(" ");
  let path = `M ${points[0].x} ${points[0].y}`;
  for (let index = 1; index < points.length - 1; index += 1) {
    const [before, corner, after] = [points[index - 1], points[index], points[index + 1]];
    const inLength = Math.hypot(corner.x - before.x, corner.y - before.y);
    const outLength = Math.hypot(after.x - corner.x, after.y - corner.y);
    const r = Math.min(radius, inLength / 2, outLength / 2);
    if (r <= 0) { path += ` L ${corner.x} ${corner.y}`; continue; }
    const start = { x: corner.x - ((corner.x - before.x) / inLength) * r, y: corner.y - ((corner.y - before.y) / inLength) * r };
    const end = { x: corner.x + ((after.x - corner.x) / outLength) * r, y: corner.y + ((after.y - corner.y) / outLength) * r };
    path += ` L ${start.x} ${start.y} Q ${corner.x} ${corner.y} ${end.x} ${end.y}`;
  }
  const last = points[points.length - 1];
  return `${path} L ${last.x} ${last.y}`;
}

/** Where an edge label sits: the middle of the route's longest segment. */
export function labelAnchor(points: readonly { x: number; y: number }[]): { x: number; y: number } {
  let best = { x: points[0].x, y: points[0].y, length: -1 };
  for (let index = 1; index < points.length; index += 1) {
    const [from, to] = [points[index - 1], points[index]];
    const length = Math.hypot(to.x - from.x, to.y - from.y);
    if (length > best.length) best = { x: (from.x + to.x) / 2, y: (from.y + to.y) / 2, length };
  }
  return { x: best.x, y: best.y };
}

function RoutedEdge({ id, data, sourceX, sourceY, targetX, targetY, markerEnd, style }: EdgeProps<RouteEdge>): JSX.Element {
  const route = data?.route;
  const points = route && route.length >= 2 ? route.map((point) => ({ ...point })) : [{ x: sourceX, y: sourceY }, { x: targetX, y: targetY }];
  if (!data?.arrow) return <BaseEdge id={id} markerEnd={markerEnd} path={points.map((point, index) => `${index === 0 ? "M" : "L"} ${point.x} ${point.y}`).join(" ")} style={style} />;
  const anchor = labelAnchor(points);
  // Design arrowhead: the line stops short of the card and a filled triangle points into it.
  const end = points[points.length - 1];
  const before = points[points.length - 2];
  const down = end.y >= before.y;
  // The tip stops 1 px short of the card edge.
  const tip = { x: end.x, y: end.y + (down ? -1 : 1) };
  points[points.length - 1] = { x: tip.x, y: tip.y + (down ? -ARROW.length : ARROW.length) };
  const base = tip.y + (down ? -ARROW.length : ARROW.length);
  return <>
    <BaseEdge id={id} path={roundedPath(points)} style={style} />
    <path className="routed-edge-arrow" d={`M ${tip.x - ARROW.width / 2} ${base} L ${tip.x + ARROW.width / 2} ${base} L ${tip.x} ${tip.y} Z`} fill={data.arrow} />
    {data.label ? <EdgeLabelRenderer><span className="flow-edge-label" style={{ transform: `translate(-50%, -50%) translate(${anchor.x}px, ${anchor.y}px)` }}>{data.label}</span></EdgeLabelRenderer> : null}
  </>;
}

function NodeInspector({ node, summary, factSet, findings, sources, onClose, onFinding, onSource }: { node: GraphNode; summary: PlanNodeSummary | null; factSet?: ExplainNodeFactSet; findings: readonly FindingTarget[]; sources: ReadonlyMap<string, SourceLabel>; onClose(): void; onFinding(target: FindingTarget): void; onSource(ref: string): void }): JSX.Element {
  const { text } = useLocale();
  const title = node.display_name ?? node.plan_node_type ?? node.node_id;
  const hotspot = hotspotLabel(summary, text);
  const sourceRefs = [...new Set([...node.source_refs, ...(summary?.total_time.source_refs ?? []), ...(summary?.output_rows.source_refs ?? [])])];
  return <aside className="node-inspector" aria-label={text("Selected plan node", "已选计划节点")}>
    <header><div><strong>{title}</strong><span>#{node.plan_node_id ?? node.node_id}{summary?.fragment_ref ? ` · Fragment ${summary.fragment_ref}` : node.fragment_id ? ` · Fragment ${node.fragment_id}` : ""}</span></div><button className="graph-tool" onClick={onClose} type="button">{text("Close", "关闭")}</button></header>
    {summary ? <dl className="inspector-metrics">
      <div><dt>{text("Time", "耗时")}</dt><dd>{summary.total_time.display_text ?? text("Unavailable", "不可用")}</dd></div>
      <div><dt>{text("Share of operator time", "占算子总耗时")}</dt><dd>{summary.time_share_display ?? "—"}{hotspot ? ` · ${hotspot}` : ""}</dd></div>
      <div><dt>{text("Output rows", "输出行数")}</dt><dd>{summary.output_rows.display_text ?? text("Unavailable", "不可用")}</dd></div>
    </dl> : null}
    {findings.length ? <section className="inspector-findings"><h4>{text("Findings on this node", "此节点的发现")}</h4><ul>{findings.map((target) => <li key={target.findingId}><button className="reference-button" onClick={() => onFinding(target)} type="button">{target.severity} · {target.title}</button></li>)}</ul></section> : null}
    {(node.sql_facts ?? []).length ? <section><h4>{text("Plan details", "计划细节")}</h4><dl className="inspector-facts">{(node.sql_facts ?? []).map((fact) => <div key={`${fact.kind}:${fact.value}`}><dt>{sqlFactLabel(fact.kind, text)}</dt><dd>{fact.value}</dd></div>)}</dl></section> : null}
    {factSet ? <section><h4>{text("Plan estimates", "计划估算")}</h4><dl className="inspector-facts">{factSet.facts.map((fact) => <div key={fact.fact_ref}><dt>{explainFactLabel(fact.fact_kind, text)}</dt><dd>{fact.state === "available" ? explainFactValue(fact, text) : text("Unavailable", "不可用")}</dd></div>)}</dl></section> : null}
    <NodeSqlLocation node={node} />
    {summary?.operators.length ? <section><h4>{text("Runtime operators", "运行时算子")}</h4><table className="inspector-operators"><thead><tr><th>{text("Operator", "算子")}</th><th>{text("Time", "耗时")}</th><th>{text("Rows in → out", "行数 入→出")}</th></tr></thead><tbody>{summary.operators.map((operator, index) => <tr key={`${operator.operator_type}:${index}`}><td title={operator.role === "subordinate" ? text("Subordinate operator", "从属算子") : undefined}>{operator.operator_type}{operator.role === "subordinate" ? <small> · {text("aux", "辅助")}</small> : null}{operator.fragment_ref ? <small> · F{operator.fragment_ref}</small> : null}</td><td>{operator.operator_time.display_text ?? "—"}</td><td>{operator.push_rows.display_text ?? "—"} → {operator.pull_rows.display_text ?? "—"}</td></tr>)}</tbody></table></section> : null}
    {summary?.reasons.length ? <p className="inspector-note">{text("Summary limits", "汇总限制")}: {summary.reasons.map((reason) => localizedEnum(reason, text)).join(" · ")}</p> : null}
    {sourceRefs.length ? <details className="inspector-source"><summary>{text("Original Profile lines", "Profile 原文行")} · {sourceRefs.length}</summary><div className="source-ref-list inspector-source-lines">{sourceRefs.map((ref, index) => <button className="reference-button" key={ref} onClick={() => onSource(ref)} title={ref} type="button">{sourceRefText(sources.get(ref), index, text)}</button>)}</div></details> : null}
  </aside>;
}

type SourceLabel = { lineNo: number | null; text: string };

/** Joins a source reference to the backend source view by whole-string equality; the reference text is never parsed. */
function sourceLabels(view: SourceView | null): Map<string, SourceLabel> {
  const labels = new Map<string, SourceLabel>();
  for (const line of view?.plain_text_lines ?? []) if (!labels.has(line.source_ref)) labels.set(line.source_ref, { lineNo: line.line_no, text: line.text.trim() });
  // A line outside the safe allowlist is named by its number only; its text is never sent.
  for (const line of view?.line_refs ?? []) if (!labels.has(line.source_ref)) labels.set(line.source_ref, { lineNo: line.line_no, text: "" });
  for (const token of view?.json_tokens ?? []) if (!labels.has(token.source_ref)) labels.set(token.source_ref, { lineNo: null, text: token.text.trim() });
  return labels;
}

function sourceRefText(label: SourceLabel | undefined, index: number, text: LocalizedText): string {
  if (!label) return text(`Source reference ${index + 1}`, `来源引用 ${index + 1}`);
  if (label.lineNo === null) return label.text;
  return `${text(`Line ${label.lineNo}`, `第 ${label.lineNo} 行`)}${label.text ? ` · ${label.text}` : ""}`;
}

export function sqlFactLabel(kind: GraphSqlFact["kind"], text: LocalizedText): string {
  switch (kind) {
    case "table": return text("Table", "表");
    case "fields": return text("Fields", "字段");
    case "predicate": return text("Filter", "过滤条件");
    case "join_condition": return text("Join condition", "关联条件");
  }
}

