import ELK, { type ELK as ElkLayoutEngine, type ElkExtendedEdge, type ElkNode, type ElkPoint } from "elkjs/lib/elk-api.js";

export type LayoutInput = {
  nodes: { id: string }[];
  edges: { id: string; source: string; target: string; reverseForLayout: boolean }[];
  groups: { id: string; nodeIds: string[] }[];
  nodeWidth: number;
  nodeHeight: number;
  density?: "standard" | "dense";
  /** Room between layers for a label on each edge (Doctor data flow). */
  edgeLabels?: boolean;
};

export type LayoutResult = {
  positions: Record<string, { x: number; y: number }>;
  groups: { id: string; x: number; y: number; width: number; height: number }[];
  routes: Record<string, { x: number; y: number }[]>;
  width: number;
  height: number;
};

export const LAYOUT_BUDGET = { nodes: 500, edges: 1000 } as const;

export type LayoutErrorCode = "layout_invalid_input" | "layout_budget_exceeded" | "layout_elk_type_error" | "layout_elk_reference_error" | "layout_elk_error" | "layout_elk_failed" | "layout_invalid_geometry" | "layout_missing_group" | "layout_missing_node" | "layout_missing_route" | "layout_route_direction" | "layout_failed";

class LayoutError extends Error {
  constructor(readonly code: LayoutErrorCode, message: string) {
    super(message);
    this.name = "LayoutError";
  }
}

type Point = { x: number; y: number };
type ValidatedInput = { input: LayoutInput; nodeIds: Set<string>; memberships: Map<string, string> };

const ROOT_ID = "execution-layout-root";
const EPSILON = 0.01;

function invalid(message: string): never {
  throw new LayoutError("layout_invalid_input", message);
}

function finitePositive(value: number, name: string): void {
  if (!Number.isFinite(value) || value <= 0) invalid(`${name} must be a finite positive number`);
}

function nonEmptyId(value: unknown, name: string): asserts value is string {
  if (typeof value !== "string" || value.length === 0) invalid(`${name} must be a non-empty string`);
}

function validateInput(input: LayoutInput): ValidatedInput {
  if (!input || !Array.isArray(input.nodes) || !Array.isArray(input.edges) || !Array.isArray(input.groups)) invalid("layout input arrays are required");
  finitePositive(input.nodeWidth, "nodeWidth");
  finitePositive(input.nodeHeight, "nodeHeight");
  if (input.density !== undefined && input.density !== "standard" && input.density !== "dense") invalid("density must be standard or dense");
  if (input.edgeLabels !== undefined && typeof input.edgeLabels !== "boolean") invalid("edgeLabels must be boolean");
  if (input.nodes.length > LAYOUT_BUDGET.nodes || input.edges.length > LAYOUT_BUDGET.edges) {
    throw new LayoutError("layout_budget_exceeded", `layout budget is ${LAYOUT_BUDGET.nodes} nodes and ${LAYOUT_BUDGET.edges} edges`);
  }

  const nodeIds = new Set<string>();
  for (const node of input.nodes) {
    nonEmptyId(node?.id, "node id");
    if (nodeIds.has(node.id)) invalid("duplicate node id");
    nodeIds.add(node.id);
  }

  const edgeIds = new Set<string>();
  for (const edge of input.edges) {
    nonEmptyId(edge?.id, "edge id");
    nonEmptyId(edge?.source, "edge source");
    nonEmptyId(edge?.target, "edge target");
    if (typeof edge.reverseForLayout !== "boolean") invalid("edge reverseForLayout must be boolean");
    if (edgeIds.has(edge.id)) invalid("duplicate edge id");
    if (!nodeIds.has(edge.source) || !nodeIds.has(edge.target)) invalid("edge endpoint does not exist");
    edgeIds.add(edge.id);
  }

  const groupIds = new Set<string>();
  const memberships = new Map<string, string>();
  for (const group of input.groups) {
    nonEmptyId(group?.id, "group id");
    if (!Array.isArray(group.nodeIds) || group.nodeIds.length === 0) invalid("group must have at least one member");
    if (groupIds.has(group.id) || nodeIds.has(group.id)) invalid("duplicate graph element id");
    groupIds.add(group.id);
    const memberIds = new Set<string>();
    for (const nodeId of group.nodeIds) {
      nonEmptyId(nodeId, "group member id");
      if (!nodeIds.has(nodeId) || memberIds.has(nodeId) || memberships.has(nodeId)) invalid("unknown or duplicate group member");
      memberIds.add(nodeId);
      memberships.set(nodeId, group.id);
    }
  }
  return { input, nodeIds, memberships };
}

function rootOptions(dense: boolean, labels: boolean): Record<string, string> {
  return {
    "elk.algorithm": "layered",
    "elk.direction": "DOWN",
    "elk.edgeRouting": "ORTHOGONAL",
    "elk.hierarchyHandling": "INCLUDE_CHILDREN",
    "elk.padding": "[top=20,left=20,bottom=20,right=20]",
    "elk.spacing.nodeNode": dense ? "32" : "48",
    "elk.layered.spacing.nodeNodeBetweenLayers": labels ? "72" : dense ? "32" : "48",
    "elk.layered.nodePlacement.strategy": "NETWORK_SIMPLEX",
    "elk.layered.crossingMinimization.strategy": "LAYER_SWEEP",
    "elk.layered.cycleBreaking.strategy": "GREEDY",
  };
}

function groupOptions(dense: boolean, labels: boolean): Record<string, string> {
  return {
    "elk.algorithm": "layered",
    "elk.direction": "DOWN",
    "elk.edgeRouting": "ORTHOGONAL",
    "elk.padding": labels ? "[top=52,left=28,bottom=28,right=28]" : "[top=40,left=20,bottom=20,right=20]",
    "elk.spacing.nodeNode": labels ? "40" : dense ? "16" : "24",
    "elk.layered.spacing.nodeNodeBetweenLayers": labels ? "56" : dense ? "18" : "22",
    "elk.layered.nodePlacement.strategy": "NETWORK_SIMPLEX",
    "elk.layered.crossingMinimization.strategy": "LAYER_SWEEP",
  };
}

function elkEdge(edge: LayoutInput["edges"][number]): ElkExtendedEdge {
  const source = edge.reverseForLayout ? edge.target : edge.source;
  const target = edge.reverseForLayout ? edge.source : edge.target;
  return { id: edge.id, sources: [source], targets: [target] };
}

function buildElkGraph({ input, memberships }: ValidatedInput): ElkNode {
  const dense = input.density === "dense";
  const labels = input.edgeLabels === true;
  const groupEdges = new Map<string, ElkExtendedEdge[]>(input.groups.map((group) => [group.id, []]));
  const rootEdges: ElkExtendedEdge[] = [];
  for (const edge of input.edges) {
    const layoutSource = edge.reverseForLayout ? edge.target : edge.source;
    const layoutTarget = edge.reverseForLayout ? edge.source : edge.target;
    const sourceGroup = memberships.get(layoutSource);
    const targetGroup = memberships.get(layoutTarget);
    const containerEdges = sourceGroup !== undefined && sourceGroup === targetGroup ? groupEdges.get(sourceGroup) : rootEdges;
    containerEdges?.push(elkEdge(edge));
  }
  return {
    id: ROOT_ID,
    layoutOptions: rootOptions(dense, labels),
    children: [
      ...input.nodes.filter((node) => !memberships.has(node.id)).map((node) => ({ id: node.id, width: input.nodeWidth, height: input.nodeHeight })),
      ...input.groups.map((group) => ({
        id: group.id,
        layoutOptions: groupOptions(dense, labels),
        children: group.nodeIds.map((id) => ({ id, width: input.nodeWidth, height: input.nodeHeight })),
        edges: groupEdges.get(group.id),
      })),
    ],
    edges: rootEdges,
  };
}

function finitePoint(point: ElkPoint | undefined, offset: Point): Point {
  const x = (point?.x ?? Number.NaN) + offset.x;
  const y = (point?.y ?? Number.NaN) + offset.y;
  if (!Number.isFinite(x) || !Number.isFinite(y)) throw new LayoutError("layout_invalid_geometry", "ELK returned non-finite route geometry");
  return { x, y };
}

function samePoint(left: Point, right: Point): boolean {
  return Math.abs(left.x - right.x) <= EPSILON && Math.abs(left.y - right.y) <= EPSILON;
}

function flattenSections(sections: { start: Point; bends: Point[]; end: Point }[]): Point[] {
  const points: Point[] = [];
  for (const section of sections) {
    for (const point of [section.start, ...section.bends, section.end]) {
      if (!points.length || !samePoint(points.at(-1)!, point)) points.push(point);
    }
  }
  return points;
}

type CollectedLayout = { positions: Map<string, Point>; groups: Map<string, { id: string; x: number; y: number; width: number; height: number }>; routes: Map<string, Point[]> };

function collectLayout(layout: ElkNode, offset: Point, nodeIds: Set<string>, groupIds: Set<string>, collected: CollectedLayout): void {
  const x = (layout.x ?? 0) + offset.x;
  const y = (layout.y ?? 0) + offset.y;
  if (!Number.isFinite(x) || !Number.isFinite(y)) throw new LayoutError("layout_invalid_geometry", "ELK returned non-finite node geometry");
  const absolute = { x, y };
  if (layout.id && nodeIds.has(layout.id)) collected.positions.set(layout.id, absolute);
  if (layout.id && groupIds.has(layout.id)) {
    const width = layout.width ?? Number.NaN;
    const height = layout.height ?? Number.NaN;
    if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) throw new LayoutError("layout_invalid_geometry", "ELK returned invalid group geometry");
    collected.groups.set(layout.id, { id: layout.id, x, y, width, height });
  }
  for (const edge of layout.edges ?? []) {
    if (!edge.id || collected.routes.has(edge.id)) throw new LayoutError("layout_invalid_geometry", "ELK returned duplicate edge geometry");
    const sections = (edge.sections ?? []).map((section) => ({
      start: finitePoint(section.startPoint, absolute),
      bends: (section.bendPoints ?? []).map((point) => finitePoint(point, absolute)),
      end: finitePoint(section.endPoint, absolute),
    }));
    if (!sections.length) throw new LayoutError("layout_missing_route", "ELK returned an unrouted edge");
    collected.routes.set(edge.id, flattenSections(sections));
  }
  for (const child of layout.children ?? []) collectLayout(child, absolute, nodeIds, groupIds, collected);
}

function onRectangleBoundary(point: Point, position: Point, width: number, height: number): boolean {
  const withinX = point.x >= position.x - EPSILON && point.x <= position.x + width + EPSILON;
  const withinY = point.y >= position.y - EPSILON && point.y <= position.y + height + EPSILON;
  return withinX && withinY && (
    Math.abs(point.x - position.x) <= EPSILON
    || Math.abs(point.x - (position.x + width)) <= EPSILON
    || Math.abs(point.y - position.y) <= EPSILON
    || Math.abs(point.y - (position.y + height)) <= EPSILON
  );
}

function engineErrorCode(error: unknown): LayoutErrorCode {
  if (error instanceof TypeError) return "layout_elk_type_error";
  if (error instanceof ReferenceError) return "layout_elk_reference_error";
  if (error instanceof Error) return "layout_elk_error";
  return "layout_elk_failed";
}

type ElkSession = { engine: ElkLayoutEngine; release: () => void };

function releaseElk(engine: ElkLayoutEngine): void {
  try {
    engine.terminateWorker();
  } catch {
    // The result has already settled; worker shutdown must not replace it.
  }
}

type InlineElkWorker = new (url?: string) => Worker;
type InlineElkModule = { Worker?: InlineElkWorker; default?: InlineElkWorker | { Worker?: InlineElkWorker } };

async function createElk(): Promise<ElkSession> {
  // The ELK engine runs inside this already-dedicated layout worker. A nested
  // Worker added a second hop and never answers in some embedded browsers, so
  // the ELK script is loaded as its own chunk and driven in-thread.
  // Inside a worker the ELK script would claim this scope's onmessage and export
  // nothing; a placeholder document while it evaluates selects its CommonJS
  // export, the same branch it takes on a browser main thread.
  const scope = globalThis as { document?: unknown };
  const placeholder = !("document" in scope);
  if (placeholder) scope.document = {};
  let elkModule: InlineElkModule;
  try {
    elkModule = (await import("elkjs/lib/elk-worker.min.js")) as unknown as InlineElkModule;
  } finally {
    if (placeholder) delete scope.document;
  }
  const InlineWorker = elkModule.Worker ?? (typeof elkModule.default === "function" ? elkModule.default : elkModule.default?.Worker);
  if (typeof InlineWorker !== "function") throw new LayoutError("layout_elk_reference_error", "ELK in-thread worker is unavailable");
  const engine = new ELK({ workerUrl: "in-thread", workerFactory: () => new InlineWorker("in-thread") });
  return { engine, release: () => releaseElk(engine) };
}

function assertOutput(result: LayoutResult, { input, memberships }: ValidatedInput): void {
  if (!Number.isFinite(result.width) || !Number.isFinite(result.height) || result.width < 0 || result.height < 0) throw new LayoutError("layout_invalid_geometry", "ELK returned invalid canvas geometry");
  const groupsById = new Map(result.groups.map((group) => [group.id, group]));
  for (const group of input.groups) {
    const output = groupsById.get(group.id);
    if (!output || ![output.x, output.y, output.width, output.height].every(Number.isFinite) || output.width <= 0 || output.height <= 0) throw new LayoutError("layout_missing_group", "ELK omitted group geometry");
  }
  for (const node of input.nodes) {
    const position = result.positions[node.id];
    if (!position || !Number.isFinite(position.x) || !Number.isFinite(position.y)) throw new LayoutError("layout_missing_node", "ELK omitted node geometry");
    const groupId = memberships.get(node.id);
    const group = groupId ? groupsById.get(groupId) : undefined;
    if (group && (position.x < group.x - EPSILON || position.y < group.y - EPSILON || position.x + input.nodeWidth > group.x + group.width + EPSILON || position.y + input.nodeHeight > group.y + group.height + EPSILON)) {
      throw new LayoutError("layout_invalid_geometry", "ELK placed a node outside its group");
    }
  }
  for (const edge of input.edges) {
    const route = result.routes[edge.id];
    const source = result.positions[edge.source];
    const target = result.positions[edge.target];
    if (!route || route.length < 2 || route.some((point) => !Number.isFinite(point.x) || !Number.isFinite(point.y))) throw new LayoutError("layout_missing_route", "ELK omitted route geometry");
    if (!onRectangleBoundary(route[0], source, input.nodeWidth, input.nodeHeight) || !onRectangleBoundary(route.at(-1)!, target, input.nodeWidth, input.nodeHeight)) {
      throw new LayoutError("layout_route_direction", "route endpoints do not match semantic edge direction");
    }
  }
}

/** Geometry only: edge reversal affects ELK layering, never the returned source-to-target route. */
export async function layoutCompound(input: LayoutInput): Promise<LayoutResult> {
  const validated = validateInput(input);
  if (input.nodes.length === 0) return { positions: {}, groups: [], routes: {}, width: 0, height: 0 };
  let layout: ElkNode;
  let session: ElkSession | undefined;
  try {
    session = await createElk();
    layout = await session.engine.layout(buildElkGraph(validated)) as ElkNode;
  } catch (error) {
    if (error instanceof LayoutError) throw error;
    throw new LayoutError(engineErrorCode(error), "ELK layout failed");
  } finally {
    session?.release();
  }
  const collected: CollectedLayout = { positions: new Map(), groups: new Map(), routes: new Map() };
  collectLayout(layout, { x: 0, y: 0 }, validated.nodeIds, new Set(input.groups.map((group) => group.id)), collected);
  const positions = Object.fromEntries(input.nodes.map((node) => {
    const position = collected.positions.get(node.id);
    if (!position) throw new LayoutError("layout_missing_node", "ELK omitted node geometry");
    return [node.id, position];
  }));
  const routes = Object.fromEntries(input.edges.map((edge) => {
    const route = collected.routes.get(edge.id);
    if (!route) throw new LayoutError("layout_missing_route", "ELK omitted route geometry");
    return [edge.id, edge.reverseForLayout ? [...route].reverse() : route];
  }));
  const result: LayoutResult = {
    positions,
    groups: input.groups.map((group) => {
      const bounds = collected.groups.get(group.id);
      if (!bounds) throw new LayoutError("layout_missing_group", "ELK omitted group geometry");
      return bounds;
    }),
    routes,
    width: layout.width ?? Number.NaN,
    height: layout.height ?? Number.NaN,
  };
  assertOutput(result, validated);
  return result;
}

export function layoutErrorCode(error: unknown): LayoutErrorCode {
  return error instanceof LayoutError ? error.code : "layout_failed";
}
