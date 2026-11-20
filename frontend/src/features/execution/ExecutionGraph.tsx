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
import { Button, SegTabs } from "../../design";
import { localizedEnum, lookupEnumLabel } from "../../i18n/enum-label";
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
/** Doctor runtime cards (v1.1 board 04): plain white operator card, 7px severity dot, 5px bottom time-share bar. */
export const GRAPH_CARD_COMPACT = { width: 244, height: 84 } as const;
/** Bottom clearance so the lowest plan nodes and arrows are never clipped by the card frame. */
export const GRAPH_BOTTOM_ROOM_PX = 56;
const ARROW = { width: 10, length: 7 } as const;
const PORT = 6;
const PLAN_CARD_HANDLES = [
  { type: "source", position: Position.Top, x: (GRAPH_CARD.width - PORT) / 2, y: -PORT / 2, width: PORT, height: PORT },
  { type: "target", position: Position.Bottom, x: (GRAPH_CARD.width - PORT) / 2, y: GRAPH_CARD.height - PORT / 2, width: PORT, height: PORT },
] as const;
const FIT_ALL_NODE_LIMIT = 12;

type SeverityDotTone = "critical" | "warn" | "hollow";

type CardData = Record<string, unknown> & {
  node: GraphNode;
  summary: PlanNodeSummary | null;
  facts: { label: string; value: string }[];
  selected: boolean;
  focused: boolean;
  findingCount: number;
  findingDotTone: SeverityDotTone | null;
  primaryFinding: FindingTarget | null;
  compact: boolean;
  barPermyriad: number;
  unconnected: boolean;
};
type CardNode = Node<CardData, "plan">;
type RouteEdge = Edge<{ route?: { x: number; y: number }[]; arrow?: string; label?: string; derived?: boolean; partial?: boolean }, "route">;

/** One rendered relation in data-flow direction (upstream → downstream). */
export type FlowEdge = { id: string; source: string; target: string; derived: boolean; partial: boolean; role: "probe" | "build" | null };

/**
 * Converts typed graph relations to data-flow direction. An edge the backend
 * marks as restating another (`restates_edge_id`) is skipped. Nothing is
 * invented or deduplicated here; only DTO edges are drawn.
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

/** Semantic operator family helper; v1.1 renders all operator cards plain white without category fills. */
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
  const provenance = edge.derived ? text("Verified derived", "已验证推导") : edge.partial ? text("Partial", "部分") : null;
  if (!rows) {
    if (provenance && side) return `${provenance} · ${side}`;
    return side ?? provenance ?? undefined;
  }
  const value = `${rows} ${text("rows", "行")}`;
  if (side) return `${side} · ${value}`;
  if (provenance) return `${provenance} · ${value}`;
  return value;
}

export function hotspotLabel(summary: PlanNodeSummary | null, text: LocalizedText): string | null {
  if (summary?.hotspot === "most_consuming") return text("Top cost", "最耗时");
  if (summary?.hotspot === "second_most_consuming") return text("High cost", "次耗时");
  if (summary?.hotspot === "boundary") return text("Near threshold", "临界");
  return null;
}

function targetSeverityRank(target: FindingTarget): number {
  const raw = (target.severityCode ?? target.severity).toLowerCase();
  if (raw === "critical" || raw === "severe" || raw === "high" || raw === "严重" || raw === "高") return 0;
  if (raw === "moderate" || raw === "medium" || raw === "中等" || raw === "中") return 1;
  return 2;
}

function severityDotOf(targets: readonly FindingTarget[]): SeverityDotTone | null {
  if (targets.length === 0) return null;
  let best = 3;
  for (const target of targets) {
    const rank = targetSeverityRank(target);
    if (rank < best) best = rank;
  }
  return best === 0 ? "critical" : best === 1 ? "warn" : "hollow";
}

function primaryFindingTarget(targets: readonly FindingTarget[]): FindingTarget | null {
  let best: FindingTarget | null = null;
  let bestRank = 4;
  for (const target of targets) {
    const rank = targetSeverityRank(target);
    if (!best || rank < bestRank) {
      best = target;
      bestRank = rank;
    }
  }
  return best;
}

export function findingDotLabel(primaryFinding: FindingTarget, findingCount: number, text: LocalizedText): string {
  const base = `${primaryFinding.severity} · ${primaryFinding.title}`;
  return findingCount > 1 ? `${base} · ${text(`${findingCount} findings`, `${findingCount} 条发现`)}` : base;
}

/** Initial placement runs again on every layout or canvas-size change; an explicit target ("在图中定位") wins. */
export function shouldAutoFitGraphOnMount(params: { hasLayout: boolean; canvasWidth: number; explicitNodeId: string | null }): boolean {
  return params.hasLayout && params.canvasWidth > 0 && params.explicitNodeId === null;
}

export function resolveGraphCenterKey(params: {
  hasLayout: boolean;
  nodeId: string | null;
  revealCount: number;
  canvasWidth: number;
  canvasHeight: number;
  viewportInitialized?: boolean;
  previousKey: string | null;
}): string | null {
  if (!params.nodeId) return null;
  if (
    !params.hasLayout
    || params.canvasWidth <= 0
    || params.canvasHeight <= 0
    || params.viewportInitialized === false
  ) {
    return params.previousKey;
  }
  return `${params.nodeId}:${params.revealCount}`;
}

export function shouldExitExpandedGraphOnKey(
  event: Pick<KeyboardEvent, "key" | "defaultPrevented">,
  hasOpenModal: boolean,
): boolean {
  return event.key === "Escape" && !event.defaultPrevented && !hasOpenModal;
}

const nodeTypes = { plan: PlanCard };
const edgeTypes = { route: RoutedEdge };

export type ExecutionGraphProps = {
  graph: GraphView;
  summary?: ExecutionSummary | null;
  factSets?: readonly ExplainNodeFactSet[];
  graphKey: string;
  findingTargets?: readonly FindingTarget[];
  defaultFindingId?: string | null;
  sourceView?: SourceView | null;
  viewExtra?: ReactNode;
  /** Embedded inside Doctor's multi-tab card (omits redundant outer card border/title). */
  embedded?: boolean;
};

type GraphGeometry = { positions: Record<string, { x: number; y: number }>; groups: { id: string; x: number; y: number; width: number; height: number }[]; routes: Record<string, { x: number; y: number }[]> };

export type FindingTarget = { findingId: string; subjectRef: string | null; nodeId: string; title: string; severity: string; severityCode?: string };

const READABLE_ZOOM = 0.6;
const FIT_PADDING = { top: "20px", right: "24px", bottom: `${GRAPH_BOTTOM_ROOM_PX}px`, left: "24px" } as const;

export function ExecutionGraph(props: ExecutionGraphProps): JSX.Element {
  return <ExecutionGraphKeyed {...props} key={props.graphKey} />;
}

function ExecutionGraphKeyed(props: ExecutionGraphProps): JSX.Element {
  const [view, setView] = useState<"flow" | "plan">("flow");
  const model = useGraphElements(props, view);
  return (
    <ReactFlowProvider initialEdges={model.flow} initialNodes={model.nodes}>
      <GraphCanvas {...props} model={model} onViewChange={setView} view={view} />
    </ReactFlowProvider>
  );
}

type GraphElementsModel = ReturnType<typeof useGraphElements>;

function useGraphElements(
  { graph, summary, factSets, findingTargets = [], defaultFindingId = null, sourceView = null }: ExecutionGraphProps,
  view: "flow" | "plan",
) {
  const { text } = useLocale();
  const { nodeId, findingId } = useSelection();
  const doctorView = summary !== undefined && summary !== null && !(factSets?.length);
  const flowView = doctorView && view === "flow";
  const card = doctorView ? GRAPH_CARD_COMPACT : GRAPH_CARD;
  const [geometry, setGeometry] = useState<{ input: LayoutInput; result?: LayoutResult; error?: string } | null>(null);

  const summaries = useMemo(() => new Map((summary?.state === "available" ? summary.nodes : []).map((node) => [node.node_id, node] as const)), [summary]);
  const factsByNode = useMemo(() => new Map((factSets ?? []).map((set) => [set.node_ref, set] as const)), [factSets]);
  const sources = useMemo(() => sourceLabels(sourceView), [sourceView]);
  const edges = useMemo(() => flowEdges(graph), [graph]);
  const groups = useMemo(() => fragmentGroups(graph, summaries), [graph, summaries]);
  const hottest = useMemo(() => hottestNodeId(summary), [summary]);
  const activeFindingId = findingId ?? defaultFindingId;
  const focusNodeId = findingTargets.find((target) => target.findingId === activeFindingId)?.nodeId ?? null;
  const findingsByNode = useMemo(() => {
    const map = new Map<string, FindingTarget[]>();
    for (const target of findingTargets) {
      const list = map.get(target.nodeId);
      if (list) list.push(target);
      else map.set(target.nodeId, [target]);
    }
    return map;
  }, [findingTargets]);

  const tree = useMemo<DataFlowLayout | null>(() => flowView && groups.length === 0 ? dataFlowLayout({
    nodes: graph.nodes.map((node) => ({ id: node.node_id, planNodeId: node.plan_node_id })),
    edges, unconnected: graph.unconnected_node_ids, card,
  }) : null, [card, edges, flowView, graph.nodes, graph.unconnected_node_ids, groups.length]);

  const layoutInput = useMemo<LayoutInput>(() => ({
    nodes: graph.nodes.map((node) => ({ id: node.node_id })),
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

  const treeLayout = useMemo<GraphGeometry | undefined>(
    () => (tree ? { positions: tree.positions, routes: tree.routes, groups: [] } : undefined),
    [tree],
  );
  const layout: GraphGeometry | undefined = treeLayout ?? (geometry?.input === layoutInput ? geometry.result : undefined);
  const layoutError = tree ? undefined : geometry?.input === layoutInput ? geometry.error : undefined;
  const maxShare = useMemo(() => Math.max(0, ...[...summaries.values()].map((node) => node.time_share_basis_points ?? 0)), [summaries]);
  const unconnected = useMemo(() => new Set(graph.unconnected_node_ids), [graph.unconnected_node_ids]);

  const nodes = useMemo<CardNode[]>(() => layout ? graph.nodes.map((node) => {
    const nodeFindings = findingsByNode.get(node.node_id) ?? [];
    return {
      id: node.node_id,
      type: "plan",
      position: layout.positions[node.node_id] ?? { x: 0, y: 0 },
      width: card.width,
      height: card.height,
      handles: (flowView ? flowHandles(card) : planHandles(card)).map((handle) => ({ ...handle })),
      draggable: false,
      selectable: true,
      data: {
        node,
        summary: summaries.get(node.node_id) ?? null,
        facts: explainCardFacts(factsByNode.get(node.node_id), text),
        selected: node.node_id === nodeId,
        focused: node.node_id === focusNodeId,
        findingCount: nodeFindings.length,
        findingDotTone: severityDotOf(nodeFindings),
        primaryFinding: primaryFindingTarget(nodeFindings),
        compact: doctorView,
        barPermyriad: maxShare > 0 ? Math.round((summaries.get(node.node_id)?.time_share_basis_points ?? 0) * 10000 / maxShare) : 0,
        unconnected: unconnected.has(node.node_id),
      },
    };
  }) : [], [card, doctorView, factsByNode, findingsByNode, flowView, focusNodeId, graph.nodes, layout, maxShare, nodeId, summaries, text, unconnected]);

  const flow = useMemo<RouteEdge[]>(() => layout ? edges.map((edge) => {
    const color = edge.partial ? "var(--warning-dot)" : edge.derived ? "var(--accent)" : "#64748b";
    return {
      id: edge.id,
      source: edge.source,
      target: edge.target,
      type: "route",
      data: {
        route: layout.routes[edge.id],
        arrow: doctorView ? color : undefined,
        label: flowView ? flowEdgeLabel(edge, summaries.get(edge.source), text) : undefined,
        derived: edge.derived,
        partial: edge.partial,
      },
      markerEnd: doctorView ? undefined : { type: MarkerType.ArrowClosed, color, width: 14, height: 14 },
      style: {
        stroke: color,
        strokeWidth: 1.6,
        strokeDasharray: edge.partial ? "2 4" : edge.derived ? "6 4" : undefined,
      },
    };
  }) : [], [doctorView, edges, flowView, layout, summaries, text]);

  return {
    doctorView,
    flowView,
    card,
    summaries,
    factsByNode,
    sources,
    groups,
    hottest,
    focusNodeId,
    tree,
    layout,
    layoutError,
    unconnected,
    nodes,
    flow,
  };
}

function GraphCanvas({
  graph,
  summary,
  findingTargets = [],
  viewExtra = null,
  embedded = false,
  model,
  view,
  onViewChange,
}: ExecutionGraphProps & {
  model: GraphElementsModel;
  view: "flow" | "plan";
  onViewChange(next: "flow" | "plan"): void;
}): JSX.Element {
  const { text } = useLocale();
  const { nodeId, revealCount, findingOpen, selectNode, selectFinding, clearSelection, selectSourceRef } = useSelection();
  const { fitView, getZoom, setCenter, setViewport, viewportInitialized, zoomIn, zoomOut } = useReactFlow<CardNode, RouteEdge>();
  const canvasWidth = useStore((state) => state.width);
  const canvasHeight = useStore((state) => state.height);
  const [expanded, setExpanded] = useState(false);
  const sectionRef = useRef<HTMLElement>(null);
  const centeredKeyRef = useRef<string | null>(null);
  const lastRevealRef = useRef(0);
  const nodeIdRef = useRef(nodeId);
  const findingOpenRef = useRef(findingOpen);
  const {
    doctorView,
    flowView,
    card,
    summaries,
    factsByNode,
    sources,
    groups,
    hottest,
    focusNodeId,
    tree,
    layout,
    layoutError,
    unconnected,
    nodes,
    flow,
  } = model;

  const centerOn = useCallback((id: string, zoom?: number) => {
    const point = layout?.positions[id];
    if (point) void setCenter(point.x + card.width / 2, point.y + card.height / 2, { zoom: zoom ?? 0.9, duration: 200 });
  }, [card, layout, setCenter]);

  const placeTree = useCallback((duration = 0): boolean => {
    if (!viewportInitialized || !tree || canvasWidth <= 0 || tree.bounds.width + 40 > canvasWidth || tree.bounds.height + 20 + GRAPH_BOTTOM_ROOM_PX > Math.max(canvasHeight, 1)) return false;
    void setViewport({ x: Math.round(canvasWidth / 2 - tree.rootCenterX), y: 20 - tree.bounds.y, zoom: 1 }, { duration });
    return true;
  }, [canvasHeight, canvasWidth, setViewport, tree, viewportInitialized]);

  const fitAll = useCallback(() => {
    if (!placeTree(200)) void fitView({ padding: FIT_PADDING, duration: 200 });
  }, [fitView, placeTree]);

  const focusReadable = useCallback((id: string, zoom: number) => {
    const point = layout?.positions[id];
    if (!point || canvasWidth <= 0 || canvasHeight <= 0) { centerOn(id, zoom); return; }
    const top = Math.min(...Object.values(layout.positions).map((position) => position.y));
    const centeredY = (canvasHeight - GRAPH_BOTTOM_ROOM_PX) / 2 - (point.y + card.height / 2) * zoom;
    void setViewport({ x: canvasWidth / 2 - (point.x + card.width / 2) * zoom, y: Math.min(centeredY, 20 - top * zoom), zoom }, { duration: 0 });
  }, [canvasHeight, canvasWidth, card, centerOn, layout, setViewport]);

  const focusReadableRef = useRef(focusReadable);
  useLayoutEffect(() => {
    focusReadableRef.current = focusReadable;
  }, [focusReadable]);

  useLayoutEffect(() => {
    nodeIdRef.current = nodeId;
  }, [nodeId]);

  useLayoutEffect(() => {
    findingOpenRef.current = findingOpen;
  }, [findingOpen]);

  const initialFocus = useRef<string | null>(null);
  useLayoutEffect(() => {
    initialFocus.current = focusNodeId ?? hottest ?? graph.root_node_ids[0] ?? null;
  });
  useEffect(() => {
    if (!shouldAutoFitGraphOnMount({ hasLayout: Boolean(layout), canvasWidth, explicitNodeId: nodeIdRef.current })) return;
    const timer = window.setTimeout(() => {
      if (nodeIdRef.current !== null) return;
      const target = initialFocus.current;
      if (placeTree()) return;
      if (graph.nodes.length <= FIT_ALL_NODE_LIMIT || !target) {
        void fitView({ padding: FIT_PADDING, maxZoom: 1, duration: 0 }).then(() => {
          if (nodeIdRef.current === null && target && getZoom() < READABLE_ZOOM) focusReadableRef.current(target, 0.85);
        });
      } else focusReadableRef.current(target, 0.85);
    }, 0);
    return () => window.clearTimeout(timer);
  }, [canvasWidth, fitView, getZoom, graph.nodes.length, layout, placeTree]);

  const focusedFor = useRef<string | null>(null);
  useEffect(() => {
    if (!layout || !focusNodeId || canvasWidth <= 0 || canvasHeight <= 0 || !viewportInitialized) return;
    if (focusedFor.current === null) { focusedFor.current = focusNodeId; return; }
    if (focusedFor.current === focusNodeId) return;
    focusedFor.current = focusNodeId;
    if (!nodeId && layout.positions[focusNodeId]) centerOn(focusNodeId, Math.max(getZoom(), 0.85));
  }, [canvasHeight, canvasWidth, centerOn, focusNodeId, getZoom, layout, nodeId, viewportInitialized]);

  useEffect(() => {
    if (!nodeId) {
      centeredKeyRef.current = null;
      lastRevealRef.current = revealCount;
      return;
    }
    const nextKey = resolveGraphCenterKey({
      hasLayout: Boolean(layout),
      nodeId,
      revealCount,
      canvasWidth,
      canvasHeight,
      viewportInitialized,
      previousKey: centeredKeyRef.current,
    });
    if (!layout || nextKey === centeredKeyRef.current) return;
    const shouldFocusDom = revealCount !== lastRevealRef.current || (centeredKeyRef.current === null && revealCount > 0);
    centeredKeyRef.current = nextKey;
    lastRevealRef.current = revealCount;
    if (layout.positions[nodeId]) {
      centerOn(nodeId);
      if (shouldFocusDom && sectionRef.current) {
        for (const el of sectionRef.current.querySelectorAll<HTMLElement>("[data-node-id]")) {
          if (el.getAttribute("data-node-id") === nodeId) {
            el.focus({ preventScroll: true });
            break;
          }
        }
      }
    }
  }, [canvasHeight, canvasWidth, centerOn, layout, nodeId, revealCount, viewportInitialized]);

  useLayoutEffect(() => {
    if (!expanded) return;
    const overflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    const onKey = (event: KeyboardEvent) => {
      const hasOpenModal = findingOpenRef.current || Boolean(document.querySelector('[role="dialog"]'));
      if (shouldExitExpandedGraphOnKey(event, hasOpenModal)) setExpanded(false);
    };
    window.addEventListener("keydown", onKey);
    return () => { document.body.style.overflow = overflow; window.removeEventListener("keydown", onKey); };
  }, [expanded]);

  const previousExpanded = useRef(expanded);
  useEffect(() => {
    if (previousExpanded.current === expanded) return;
    previousExpanded.current = expanded;
    if (layout) window.setTimeout(() => { if (!placeTree()) void fitView({ padding: FIT_PADDING, duration: 0 }); }, 0);
  }, [expanded, fitView, layout, placeTree]);

  const selected = graph.nodes.find((node) => node.node_id === nodeId) ?? null;
  const hotspotCount = [...summaries.values()].filter((node) => node.hotspot === "most_consuming" || node.hotspot === "second_most_consuming").length;
  const fragmentCount = groups.length || new Set(graph.nodes.map((node) => node.fragment_id).filter(Boolean)).size;
  const unresolved = graph.unresolved_relations.length;
  const counts = `${graph.nodes.length} ${text("nodes", "节点")}${!doctorView && fragmentCount ? ` · ${fragmentCount} Fragment` : ""}${summary?.state === "available" && hotspotCount ? ` · ${hotspotCount} ${text("high-cost", "高耗时")}` : ""}`;

  return (
    <section
      aria-label={doctorView ? text("Execution structure", "执行结构") : text("Execution graph", "执行图")}
      className={`execution-graph${embedded ? " is-embedded" : " card"}${doctorView ? " is-doctor" : ""}${expanded ? " is-expanded" : ""}`}
      ref={sectionRef}
    >
      <header className="execution-graph-header">
        <div className="execution-graph-header-left">
          {!embedded ? <h2 className="card-title">{doctorView ? text("Execution structure", "执行结构") : text("Execution graph", "执行图")}</h2> : null}
          {doctorView ? (
            <div className="execution-graph-views">
              <SegTabs
                aria-label={text("Structure mode", "结构图模式")}
                items={[
                  { value: "flow", label: text("Data flow", "数据流") },
                  { value: "plan", label: text("Plan tree", "原始结构") },
                ]}
                onValueChange={onViewChange}
                size="sm"
                value={view}
              />
              {viewExtra}
            </div>
          ) : null}
        </div>
        <div className="card-hd-actions">
          {hottest ? (
            <button className="graph-counts mono" onClick={() => { selectNode(hottest); centerOn(hottest); }} title={text("Focus hottest node", "定位最耗时节点")} type="button">{counts}</button>
          ) : (
            <span className="card-subtitle mono">{counts}</span>
          )}
          <div aria-label={text("Graph controls", "图控制")} className="graph-toolbar" role="toolbar">
            <Button aria-label={text("Zoom out", "缩小画布")} onClick={() => void zoomOut({ duration: 150 })} size="sm" title={text("Zoom out", "缩小")} variant="secondary">−</Button>
            <Button aria-label={text("Zoom in", "放大画布")} onClick={() => void zoomIn({ duration: 150 })} size="sm" title={text("Zoom in", "放大")} variant="secondary">+</Button>
            <Button onClick={fitAll} size="sm" variant="secondary">{text("Fit all", "适配全图")}</Button>
            <Button
              aria-label={expanded ? text("Exit full screen", "退出全屏") : text("Full screen", "全屏")}
              aria-pressed={expanded}
              onClick={() => setExpanded((value) => !value)}
              size="sm"
              variant="secondary"
            >
              {expanded ? "⤡" : "⤢"}
            </Button>
          </div>
        </div>
      </header>

      <div
        className={`graph-canvas execution-graph-canvas${selected ? " has-inspector" : ""}`}
        data-bottom-room={GRAPH_BOTTOM_ROOM_PX}
        data-layout-state={layout ? "ready" : layoutError ? "failed" : "loading"}
        data-view={flowView ? "flow" : "plan"}
      >
        {graph.nodes.length === 0 ? (
          <p className="execution-graph-status" role="status">
            {text("No execution structure could be read from this Profile; check the Profile notice and evidence gaps.", "无法从此 Profile 读取执行结构；原因见 Profile 提示与证据缺口。")}
          </p>
        ) : !layout ? (
          <p className="execution-graph-status" role="status">
            {layoutError ? text("Layout unavailable; the Operators tab still lists every plan node.", "布局不可用；算子标签页仍列出全部计划节点。") : text("Arranging the plan…", "正在排列计划…")}
          </p>
        ) : null}

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
          {layout ? (
            <ViewportPortal>
              {layout.groups.map((frame) => {
                const group = groups.find((item) => item.id === frame.id);
                return (
                  <div aria-hidden="true" className="fragment-frame" key={frame.id} style={{ left: frame.x, top: frame.y, width: frame.width, height: frame.height }}>
                    <span>Fragment {group?.fragment}</span>
                  </div>
                );
              })}
            </ViewportPortal>
          ) : null}
        </ReactFlow>

        {selected ? (
          <NodeInspector
            factSet={factsByNode.get(selected.node_id)}
            findings={findingTargets.filter((target) => target.nodeId === selected.node_id)}
            node={selected}
            onClose={() => clearSelection()}
            onFinding={(target) => selectFinding(target.findingId, target.subjectRef)}
            onSource={selectSourceRef}
            sources={sources}
            summary={summaries.get(selected.node_id) ?? null}
          />
        ) : null}
      </div>

      <footer className="card-ft execution-graph-legend">
        <div className="graph-legend-bar">
          <span><i className="edge-sample" />{doctorView ? text("Explicit relation", "显式关系") : text("Data flow", "数据流向")}</span>
          <span><i className="edge-sample edge-sample--derived" />{text("Verified derived", "已验证推导")}</span>
          {doctorView ? <span><i className="edge-sample edge-sample--partial" />{text("Partial", "部分证实")}</span> : null}
          {unconnected.size > 0 ? <span>· {text(`${unconnected.size} unconnected`, `${unconnected.size} 处未连线`)}</span> : null}
          {unresolved > 0 && graph.state !== "unavailable" ? (
            <span className="legend-warning">· {unresolved} {text("topology contradiction(s) · graph partial", "处与官方拓扑不符 · 图标为部分")}</span>
          ) : null}
        </div>
        {summary?.state === "available" ? (
          <div className="graph-legend-bar">
            <span className="status-inline"><span aria-hidden="true" className="status-dot heat-dot-high" />&gt;30%</span>
            <span className="status-inline"><span aria-hidden="true" className="status-dot heat-dot-mid" />15%–30%</span>
          </div>
        ) : null}
      </footer>
    </section>
  );
}

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

function compactCardLine(node: GraphNode, summary: PlanNodeSummary | null, unconnected: boolean, text: LocalizedText): string {
  if (unconnected) return text("Up/downstream unproven · no edge drawn", "上下游未证实 · 不伪造连线");
  const fact = primarySqlFact(node) ?? (node.sql_facts ?? []).find((candidate) => candidate.kind === "fields") ?? null;
  const rows = summary?.output_rows.display_text ?? null;
  if (fact?.kind === "table") return `${text("Table", "表")} ${fact.value.split(".").pop()}${rows ? ` · ${text("out", "输出")} ${rows} ${text("rows", "行")}` : ""}`;
  if (fact) return `${fact.value}${rows ? ` · ${rows} ${text("rows", "行")}` : ""}`;
  if (node.plan_node_type?.includes("SINK")) return `${text("Result output", "结果出口")}${rows ? ` · ${rows} ${text("rows", "行")}` : ""}`;
  return rows ? `${rows} ${text("rows out", "行输出")}` : node.fragment_id ? `Fragment ${node.fragment_id}` : "—";
}

function shareFillClass(summary: PlanNodeSummary | null, unconnected: boolean): string {
  if (unconnected || !summary) return "share-fill";
  if (summary.hotspot === "most_consuming") return "share-fill share-fill--high";
  if (summary.hotspot === "second_most_consuming") return "share-fill share-fill--mid";
  return "share-fill";
}

function NodeFindingDotButton({
  findingCount,
  findingDotTone,
  primaryFinding,
}: {
  findingCount: number;
  findingDotTone: SeverityDotTone | null;
  primaryFinding: FindingTarget | null;
}): JSX.Element | null {
  const { text } = useLocale();
  const { selectFinding } = useSelection();
  if (findingCount <= 0 || !findingDotTone || !primaryFinding) return null;
  return (
    <button
      aria-haspopup="dialog"
      aria-label={findingDotLabel(primaryFinding, findingCount, text)}
      className="status-inline exec-node-finding-btn nodrag nopan"
      onClick={(event) => {
        event.stopPropagation();
        selectFinding(primaryFinding.findingId, primaryFinding.subjectRef);
      }}
      onKeyDown={(event) => {
        event.stopPropagation();
      }}
      title={text(`${findingCount} finding(s) on this node`, `此节点有 ${findingCount} 条发现`)}
      type="button"
    >
      <span aria-hidden="true" className={`status-dot status-dot--${findingDotTone}`} />
      <span>{findingCount}</span>
    </button>
  );
}

function PlanCard({ data }: NodeProps<CardNode>): JSX.Element {
  const { text } = useLocale();
  if (data.compact) return <CompactPlanCard data={data} />;
  const { node, summary, facts, selected, focused, findingCount, findingDotTone, primaryFinding, unconnected } = data;
  const title = node.display_name ?? node.plan_node_type ?? node.node_id;
  const fact = primarySqlFact(node);
  const hotspot = hotspotLabel(summary, text);
  const share = summary?.time_share_basis_points ?? null;
  const classes = [
    "exec-node",
    "exec-node--explain",
    selected && "is-selected",
    focused && !selected && "is-focused",
    unconnected && "is-unproven",
  ].filter(Boolean).join(" ");

  return (
    <article className={classes} data-node-id={node.node_id} tabIndex={-1} title={hotspot ?? undefined}>
      <Handle className="plan-port" position={Position.Bottom} type="target" />
      <div className="exec-node-hd">
        <span className="exec-node-op" title={title}>{title}</span>
        <span className="exec-node-meta">
          <NodeFindingDotButton
            findingCount={findingCount}
            findingDotTone={findingDotTone}
            primaryFinding={primaryFinding}
          />
          <span>#{node.plan_node_id ?? node.node_id}</span>
        </span>
      </div>
      <div className="exec-node-fact" title={fact?.value}>{fact ? fact.value : node.fragment_id ? `Fragment ${node.fragment_id}` : "—"}</div>
      {summary ? (
        <>
          <div className="exec-node-heat">
            <span aria-hidden="true" className="share-track">
              <span className={shareFillClass(summary, unconnected)} style={{ width: `${Math.min(100, (share ?? 0) / 100)}%` }} />
            </span>
            <span className="exec-node-time">
              {summary.hotspot === "most_consuming" ? <strong>{summary.time_share_display ?? "—"}</strong> : (summary.time_share_display ?? "—")}
              {summary.total_time.display_text ? ` · ${summary.total_time.display_text}` : ""}
            </span>
          </div>
          <div className="exec-node-fact">{text("Rows", "输出行")} {summary.output_rows.display_text ?? text("unavailable", "不可用")}</div>
        </>
      ) : facts.length ? (
        <dl className="plan-card-facts">
          {facts.slice(0, 2).map((item) => <div key={item.label}><dt>{item.label}</dt><dd>{item.value}</dd></div>)}
        </dl>
      ) : (
        <div className="exec-node-fact">{text("No runtime metrics", "无运行时指标")}</div>
      )}
      <Handle className="plan-port" position={Position.Top} type="source" />
    </article>
  );
}

function CompactPlanCard({ data }: { data: CardData }): JSX.Element {
  const { text } = useLocale();
  const { node, summary, selected, focused, findingCount, findingDotTone, primaryFinding, barPermyriad, unconnected } = data;
  const title = node.display_name ?? node.plan_node_type ?? node.node_id;
  const hotspot = hotspotLabel(summary, text);
  const classes = [
    "exec-node",
    selected && "is-selected",
    focused && !selected && "is-focused",
    unconnected && "is-unproven",
  ].filter(Boolean).join(" ");

  return (
    <article
      aria-label={`${title} Plan ${node.plan_node_id ?? node.node_id}${summary?.time_share_display ? ` ${text("share of operator time", "占算子总耗时")} ${summary.time_share_display}` : ""}${findingCount ? ` · ${text(`${findingCount} finding(s)`, `${findingCount} 条发现`)}` : ""}`}
      className={classes}
      data-hotspot={summary?.hotspot ?? undefined}
      data-node-id={node.node_id}
      tabIndex={-1}
      title={hotspot ?? undefined}
    >
      <Handle className="plan-port" position={Position.Top} type="target" />
      <div className="exec-node-hd">
        <span className="exec-node-op" title={title}>{title}</span>
        <span className="exec-node-meta">
          <NodeFindingDotButton
            findingCount={findingCount}
            findingDotTone={findingDotTone}
            primaryFinding={primaryFinding}
          />
          <span>Plan {node.plan_node_id ?? node.node_id}</span>
        </span>
      </div>
      <div className="exec-node-fact">{compactCardLine(node, summary, unconnected, text)}</div>
      <div className="exec-node-heat">
        <span aria-hidden="true" className="share-track">
          <span className={shareFillClass(summary, unconnected)} style={{ width: `${unconnected || !summary ? 0 : barPermyriad / 100}%` }} />
        </span>
        <span className="exec-node-time">
          {summary ? (
            <>
              {summary.hotspot === "most_consuming" ? <strong>{summary.time_share_display ?? "—"}</strong> : (summary.time_share_display ?? "—")}
              {summary.total_time.display_text ? ` · ${summary.total_time.display_text}` : ""}
            </>
          ) : text("No runtime metrics", "无运行时指标")}
        </span>
      </div>
      <Handle className="plan-port" position={Position.Bottom} type="source" />
    </article>
  );
}

/** A polyline with its bends rounded (radius up to 12 px). */
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
  const end = points[points.length - 1];
  const before = points[points.length - 2];
  const down = end.y >= before.y;
  const tip = { x: end.x, y: end.y + (down ? -1 : 1) };
  points[points.length - 1] = { x: tip.x, y: tip.y + (down ? -ARROW.length : ARROW.length) };
  const base = tip.y + (down ? -ARROW.length : ARROW.length);
  const labelClass = data.derived ? "flow-edge-label flow-edge-label--derived" : data.partial ? "flow-edge-label flow-edge-label--partial" : "flow-edge-label";
  return (
    <>
      <BaseEdge id={id} path={roundedPath(points)} style={style} />
      <path className="routed-edge-arrow" d={`M ${tip.x - ARROW.width / 2} ${base} L ${tip.x + ARROW.width / 2} ${base} L ${tip.x} ${tip.y} Z`} fill={data.arrow} />
      {data.label ? (
        <EdgeLabelRenderer>
          <span className={labelClass} style={{ transform: `translate(-50%, -50%) translate(${anchor.x}px, ${anchor.y}px)` }}>{data.label}</span>
        </EdgeLabelRenderer>
      ) : null}
    </>
  );
}

function NodeInspector({ node, summary, factSet, findings, sources, onClose, onFinding, onSource }: { node: GraphNode; summary: PlanNodeSummary | null; factSet?: ExplainNodeFactSet; findings: readonly FindingTarget[]; sources: ReadonlyMap<string, SourceLabel>; onClose(): void; onFinding(target: FindingTarget): void; onSource(ref: string): void }): JSX.Element {
  const { text } = useLocale();
  const title = node.display_name ?? node.plan_node_type ?? node.node_id;
  const hotspot = hotspotLabel(summary, text);
  const sourceRefs = [...new Set([...node.source_refs, ...(summary?.total_time.source_refs ?? []), ...(summary?.output_rows.source_refs ?? [])])];
  return (
    <aside aria-label={text("Selected plan node", "已选计划节点")} className="node-inspector">
      <header>
        <div>
          <strong>{title}</strong>
          <span>#{node.plan_node_id ?? node.node_id}{summary?.fragment_ref ? ` · Fragment ${summary.fragment_ref}` : node.fragment_id ? ` · Fragment ${node.fragment_id}` : ""}</span>
        </div>
        <Button onClick={onClose} size="sm" variant="secondary">{text("Close", "关闭")}</Button>
      </header>
      {summary ? (
        <dl className="inspector-metrics">
          <div><dt>{text("Time", "耗时")}</dt><dd>{summary.total_time.display_text ?? text("Unavailable", "不可用")}</dd></div>
          <div><dt>{text("Share of operator time", "占算子总耗时")}</dt><dd>{summary.time_share_display ?? "—"}{hotspot ? ` · ${hotspot}` : ""}</dd></div>
          <div><dt>{text("Output rows", "输出行数")}</dt><dd>{summary.output_rows.display_text ?? text("Unavailable", "不可用")}</dd></div>
        </dl>
      ) : null}
      {findings.length ? (
        <section className="inspector-findings">
          <h4>{text("Findings on this node", "此节点的发现")}</h4>
          <ul>
            {findings.map((target) => (
              <li key={target.findingId}>
                <button className="reference-button" onClick={() => onFinding(target)} type="button">{target.severity} · {target.title}</button>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
      {(node.sql_facts ?? []).length ? (
        <section>
          <h4>{text("Plan details", "计划细节")}</h4>
          <dl className="inspector-facts">
            {(node.sql_facts ?? []).map((fact) => <div key={`${fact.kind}:${fact.value}`}><dt>{sqlFactLabel(fact.kind, text)}</dt><dd>{fact.value}</dd></div>)}
          </dl>
        </section>
      ) : null}
      {factSet ? (
        <section>
          <h4>{text("Plan estimates", "计划估算")}</h4>
          <dl className="inspector-facts">
            {factSet.facts.map((fact) => <div key={fact.fact_ref}><dt>{explainFactLabel(fact.fact_kind, text)}</dt><dd>{fact.state === "available" ? explainFactValue(fact, text) : text("Unavailable", "不可用")}</dd></div>)}
          </dl>
        </section>
      ) : null}
      <NodeSqlLocation node={node} />
      {summary?.operators.length ? (
        <section>
          <h4>{text("Runtime operators", "运行时算子")}</h4>
          <table className="inspector-operators">
            <thead>
              <tr><th>{text("Operator", "算子")}</th><th>{text("Time", "耗时")}</th><th>{text("Rows in → out", "行数 入→出")}</th></tr>
            </thead>
            <tbody>
              {summary.operators.map((operator, index) => (
                <tr key={`${operator.operator_type}:${index}`}>
                  <td title={operator.role === "subordinate" ? text("Subordinate operator", "从属算子") : undefined}>
                    {operator.operator_type}
                    {operator.role === "subordinate" ? <small> · {text("aux", "辅助")}</small> : null}
                    {operator.fragment_ref ? <small> · F{operator.fragment_ref}</small> : null}
                  </td>
                  <td>{operator.operator_time.display_text ?? "—"}</td>
                  <td>{operator.push_rows.display_text ?? "—"} → {operator.pull_rows.display_text ?? "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      ) : null}
      {summary?.reasons.length ? (
        <p className="inspector-note">{text("Summary limits", "汇总限制")}: {summary.reasons.map((reason) => localizedEnum(reason, text)).join(" · ")}</p>
      ) : null}
      {sourceRefs.length ? (
        <details className="disclosure inspector-source">
          <summary>{text("Original Profile lines", "Profile 原文行")} · {sourceRefs.length}</summary>
          <div className="source-ref-list inspector-source-lines">
            {sourceRefs.map((ref, index) => <button className="reference-button" key={ref} onClick={() => onSource(ref)} title={ref} type="button">{sourceRefText(sources.get(ref), index, text)}</button>)}
          </div>
        </details>
      ) : null}
    </aside>
  );
}

type SourceLabel = { lineNo: number | null; text: string };

function sourceLabels(view: SourceView | null): Map<string, SourceLabel> {
  const labels = new Map<string, SourceLabel>();
  for (const line of view?.plain_text_lines ?? []) if (!labels.has(line.source_ref)) labels.set(line.source_ref, { lineNo: line.line_no, text: line.text.trim() });
  for (const line of view?.line_refs ?? []) if (!labels.has(line.source_ref)) labels.set(line.source_ref, { lineNo: line.line_no, text: "" });
  for (const token of view?.json_tokens ?? []) if (!labels.has(token.source_ref)) labels.set(token.source_ref, { lineNo: null, text: token.text.trim() });
  return labels;
}

function sourceRefText(label: SourceLabel | undefined, index: number, text: LocalizedText): string {
  if (!label) return text(`Source reference ${index + 1}`, `来源引用 ${index + 1}`);
  if (label.lineNo === null) return label.text;
  return `${text(`Line ${label.lineNo}`, `第 ${label.lineNo} 行`)}${label.text ? ` · ${label.text}` : ""}`;
}

const SQL_FACT_LABELS: Record<GraphSqlFact["kind"], readonly [string, string]> = {
  table: ["Table", "表"],
  fields: ["Fields", "字段"],
  predicate: ["Filter", "过滤条件"],
  join_condition: ["Join condition", "关联条件"],
};

export function sqlFactLabel(kind: GraphSqlFact["kind"], text: LocalizedText): string {
  return lookupEnumLabel(SQL_FACT_LABELS, kind, text);
}
