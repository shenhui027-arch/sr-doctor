// Top-down data-flow layout for tree-shaped plans (design board 02 · 执行结构).
//
// Sources sit on the top row; each node is one row below its deepest input
// (longest path from a source); a consumer is centred over the inputs it
// reads, which keep the DTO's edge order from left to right. Plans that are
// not a single in-tree (an operator feeding two consumers, several sinks) are
// not handled here: the caller falls back to the general ELK layout. Nodes
// the backend could not connect are drawn beside the row of their plan-id
// neighbour and never get an edge.

export type DataFlowInput = {
  nodes: { id: string; planNodeId: string | null }[];
  /** Data-flow direction (input → consumer), in DTO order. */
  edges: { id: string; source: string; target: string }[];
  /** Nodes the backend left unconnected; drawn without edges. */
  unconnected: readonly string[];
  card: { width: number; height: number };
};

export type DataFlowLayout = {
  positions: Record<string, { x: number; y: number }>;
  /** Polyline per edge; the last point is the arrow tip on the consumer's top edge. */
  routes: Record<string, { x: number; y: number }[]>;
  /** Horizontal centre of the result (sink) column; the canvas centres on it. */
  rootCenterX: number;
  bounds: { x: number; y: number; width: number; height: number };
};

export const DATA_FLOW_SPACING = { row: 40, sibling: 168, beside: 36, inputPort: 40, bendStep: 18, bendNearest: 20 } as const;

export function dataFlowLayout(input: DataFlowInput): DataFlowLayout | null {
  const { card } = input;
  const unconnected = new Set(input.unconnected);
  const ids = new Set(input.nodes.map((node) => node.id));
  const inputs = new Map<string, string[]>();
  const consumers = new Map<string, string[]>();
  for (const edge of input.edges) {
    if (!ids.has(edge.source) || !ids.has(edge.target) || unconnected.has(edge.source) || unconnected.has(edge.target)) return null;
    inputs.set(edge.target, [...(inputs.get(edge.target) ?? []), edge.source]);
    consumers.set(edge.source, [...(consumers.get(edge.source) ?? []), edge.target]);
  }
  const connected = input.nodes.filter((node) => !unconnected.has(node.id));
  if (connected.some((node) => (consumers.get(node.id)?.length ?? 0) > 1)) return null;
  const roots = connected.filter((node) => !consumers.has(node.id));
  if (roots.length !== 1) return null;
  const root = roots[0].id;

  // Row = longest path from a source; a cycle or an unreachable node is not a tree.
  const row = new Map<string, number>();
  const visiting = new Set<string>();
  const rowOf = (id: string): number => {
    const known = row.get(id);
    if (known !== undefined) return known;
    if (visiting.has(id)) throw new Error("cycle");
    visiting.add(id);
    const value = Math.max(-1, ...(inputs.get(id) ?? []).map(rowOf)) + 1;
    visiting.delete(id);
    row.set(id, value);
    return value;
  };
  try { rowOf(root); } catch { return null; }
  if (connected.some((node) => !row.has(node.id))) return null;

  // Tidy placement: subtree widths bottom-up, then left edges top-down from the root.
  const width = new Map<string, number>();
  const widthOf = (id: string): number => {
    const children = inputs.get(id) ?? [];
    const value = children.length === 0 ? card.width : Math.max(card.width, children.reduce((sum, child) => sum + widthOf(child), 0) + DATA_FLOW_SPACING.sibling * (children.length - 1));
    width.set(id, value);
    return value;
  };
  widthOf(root);
  const centerX = new Map<string, number>();
  const place = (id: string, left: number): void => {
    const children = inputs.get(id) ?? [];
    if (children.length === 0) { centerX.set(id, left + width.get(id)! / 2); return; }
    const span = children.reduce((sum, child) => sum + width.get(child)!, 0) + DATA_FLOW_SPACING.sibling * (children.length - 1);
    let cursor = left + (width.get(id)! - span) / 2;
    for (const child of children) { place(child, cursor); cursor += width.get(child)! + DATA_FLOW_SPACING.sibling; }
    centerX.set(id, (centerX.get(children[0])! + centerX.get(children[children.length - 1])!) / 2);
  };
  place(root, 0);

  const rowY = (value: number) => value * (card.height + DATA_FLOW_SPACING.row);
  const positions: DataFlowLayout["positions"] = {};
  for (const node of connected) positions[node.id] = { x: centerX.get(node.id)! - card.width / 2, y: rowY(row.get(node.id)!) };

  // Unconnected nodes: beside the row of the connected node with the next higher plan id.
  const byPlanId = connected
    .filter((node) => node.planNodeId !== null && /^-?\d+$/.test(node.planNodeId))
    .sort((left, right) => Number(left.planNodeId) - Number(right.planNodeId));
  for (const node of input.nodes.filter((candidate) => unconnected.has(candidate.id))) {
    const own = node.planNodeId !== null && /^-?\d+$/.test(node.planNodeId) ? Number(node.planNodeId) : Number.POSITIVE_INFINITY;
    const neighbour = byPlanId.find((candidate) => Number(candidate.planNodeId) > own) ?? byPlanId.at(-1) ?? null;
    const y = neighbour ? positions[neighbour.id].y : 0;
    const right = Math.max(...Object.values(positions).filter((point) => point.y === y).map((point) => point.x + card.width));
    positions[node.id] = { x: right + DATA_FLOW_SPACING.beside, y };
  }

  const routes: DataFlowLayout["routes"] = {};
  for (const [target, sources] of inputs) {
    const top = positions[target].y;
    const middle = positions[target].x + card.width / 2;
    sources.forEach((source, index) => {
      const edge = input.edges.find((candidate) => candidate.source === source && candidate.target === target)!;
      const from = { x: positions[source].x + card.width / 2, y: positions[source].y + card.height };
      // Several inputs enter side by side; the design draws them 1 px right of an even split.
      const port = sources.length === 1 ? middle : middle + (index - (sources.length - 1) / 2) * 2 * DATA_FLOW_SPACING.inputPort + 1;
      if (from.x === port) { routes[edge.id] = [from, { x: port, y: top }]; return; }
      const bend = top - DATA_FLOW_SPACING.bendNearest - DATA_FLOW_SPACING.bendStep * (sources.length - 1 - index) + 1;
      routes[edge.id] = [from, { x: from.x, y: bend }, { x: port, y: bend }, { x: port, y: top }];
    });
  }

  const xs = Object.values(positions).map((point) => point.x);
  const ys = Object.values(positions).map((point) => point.y);
  const minX = Math.min(...xs);
  const minY = Math.min(...ys);
  return {
    positions, routes,
    rootCenterX: centerX.get(root)!,
    bounds: { x: minX, y: minY, width: Math.max(...xs) + card.width - minX, height: Math.max(...ys) + card.height - minY },
  };
}
