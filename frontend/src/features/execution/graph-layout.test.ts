import fixture from "./__fixtures__/complex-plan.json";
import designGeometry from "./__fixtures__/capture-047-elk-layout.json";
import { describe, expect, it } from "vitest";
import { LAYOUT_BUDGET, layoutCompound, type LayoutInput, type LayoutResult } from "./graph-layout";

type SanitizedFixture = {
  fixture_metadata: { source_sha256: string; production_adapter_verified: boolean; status: string };
  graph_view: {
    state: string;
    topology_source: string;
    nodes: { node_id: string; plan_node_id: string | null; fragment_id: string | null }[];
    edges: { edge_id: string; from: string; to: string; relation: string; direction_semantics: string }[];
  };
  plan_node_fact_sets: { facts: { state: string; fact_kind: string }[]; card_slots: { state: string; expected_fact_kind: string | null }[] }[];
};

const sanitized = fixture as SanitizedFixture;

function complexInput(): LayoutInput {
  const fragmentIds = [...new Set(sanitized.graph_view.nodes.map((node) => node.fragment_id).filter((fragmentId): fragmentId is string => fragmentId !== null))];
  return {
    nodeWidth: 224,
    nodeHeight: 74,
    nodes: sanitized.graph_view.nodes.map((node) => ({ id: node.node_id })),
    groups: fragmentIds.map((fragmentId) => ({ id: `fragment:${fragmentId}`, nodeIds: sanitized.graph_view.nodes.filter((node) => node.fragment_id === fragmentId).map((node) => node.node_id) })),
    edges: sanitized.graph_view.edges.map((edge) => ({
      id: edge.edge_id,
      source: edge.from,
      target: edge.to,
      reverseForLayout: ["embedded_plan", "explain_costs"].includes(sanitized.graph_view.topology_source) && edge.relation === "exchange_link" && edge.direction_semantics === "upstream_to_downstream",
    })),
  };
}

function overlaps(left: { x: number; y: number; width: number; height: number }, right: { x: number; y: number; width: number; height: number }): boolean {
  return left.x < right.x + right.width && left.x + left.width > right.x && left.y < right.y + right.height && left.y + left.height > right.y;
}

function onBoundary(point: { x: number; y: number }, rectangle: { x: number; y: number; width: number; height: number }, epsilon = 0.01): boolean {
  const withinX = point.x >= rectangle.x - epsilon && point.x <= rectangle.x + rectangle.width + epsilon;
  const withinY = point.y >= rectangle.y - epsilon && point.y <= rectangle.y + rectangle.height + epsilon;
  return withinX && withinY && (
    Math.abs(point.x - rectangle.x) <= epsilon
    || Math.abs(point.x - (rectangle.x + rectangle.width)) <= epsilon
    || Math.abs(point.y - rectangle.y) <= epsilon
    || Math.abs(point.y - (rectangle.y + rectangle.height)) <= epsilon
  );
}

function crossesRectangleInterior(route: { x: number; y: number }[], rectangle: { x: number; y: number; width: number; height: number }): boolean {
  return route.slice(1).some((end, index) => {
    const start = route[index];
    let lower = 0;
    let upper = 1;
    for (const [origin, delta, min, max] of [
      [start.x, end.x - start.x, rectangle.x, rectangle.x + rectangle.width],
      [start.y, end.y - start.y, rectangle.y, rectangle.y + rectangle.height],
    ]) {
      if (Math.abs(delta) < 0.000001) {
        if (origin <= min || origin >= max) return false;
        continue;
      }
      const first = (min - origin) / delta;
      const last = (max - origin) / delta;
      lower = Math.max(lower, Math.min(first, last));
      upper = Math.min(upper, Math.max(first, last));
    }
    return lower < upper - 0.000001;
  });
}

function nodeBounds(result: LayoutResult, input: LayoutInput, id: string) {
  return { ...result.positions[id], width: input.nodeWidth, height: input.nodeHeight };
}

// ELK runs in-thread (no nested Worker); a 45-node layout takes 1–2 s alone and
// can exceed the 5 s default while the full suite runs in parallel.
const LARGE_LAYOUT_TIMEOUT_MS = 20_000;

describe("layoutCompound", () => {
  it("lays out the sanitized 45-node, 10-fragment sample without overlapping nodes or fragments", async () => {
    const input = complexInput();
    expect(sanitized.fixture_metadata.source_sha256).toBe("e592659b7cab9059b59552caf5c490bf03fe74c44c6ca068d983204223eead0c");
    expect(sanitized.fixture_metadata).toMatchObject({ status: "production_adapter_snapshot", production_adapter_verified: true });
    expect(sanitized.graph_view.state).toBe("partial");
    expect(sanitized.plan_node_fact_sets.flatMap((set) => set.facts).filter((fact) => fact.fact_kind === "plan_child_count").every((fact) => fact.state === "unavailable")).toBe(true);
    expect(sanitized.plan_node_fact_sets.flatMap((set) => set.card_slots).filter((slot) => slot.expected_fact_kind === "plan_child_count").every((slot) => slot.state === "unavailable")).toBe(true);
    expect(input.nodes).toHaveLength(45);
    expect(input.groups).toHaveLength(10);
    expect(input.edges).toHaveLength(44);
    const result = await layoutCompound(input);
    expect(Object.keys(result.positions)).toHaveLength(45);
    expect(result.groups).toHaveLength(10);
    expect(Object.keys(result.routes)).toHaveLength(44);
    expect(result.width).toBeGreaterThan(0);
    expect(result.height).toBeGreaterThan(0);
    // The design generator is the geometry baseline, not a dump of React output.
    expect(designGeometry.source.source_sha256).toBe("c7c07eff53cccfd719fa1f0f8ca3839fc08aee49639c627c099a4044d48b34aa");
    expect(result.width).toBe(designGeometry.layout.root_bounds.width);
    expect(result.height).toBe(designGeometry.layout.root_bounds.height);
    for (const node of designGeometry.nodes) {
      const planNodeId = Number(node.node_id.replace("plan:", ""));
      const productionNode = sanitized.graph_view.nodes.find((candidate) => Number(candidate.plan_node_id) === planNodeId);
      expect(productionNode).toBeDefined();
      expect(nodeBounds(result, input, productionNode!.node_id)).toEqual(node.bounds);
    }
    for (const edge of designGeometry.edges) {
      const points = edge.path_sections.flatMap(section => [section.start_point, ...section.bend_points, section.end_point]);
      const source = sanitized.graph_view.nodes.find((node) => Number(node.plan_node_id) === Number(edge.semantic_source_plan_node_id));
      const target = sanitized.graph_view.nodes.find((node) => Number(node.plan_node_id) === Number(edge.semantic_target_plan_node_id));
      const productionEdge = sanitized.graph_view.edges.find((candidate) => candidate.from === source?.node_id && candidate.to === target?.node_id);
      expect(productionEdge).toBeDefined();
      expect(result.routes[productionEdge!.edge_id]).toEqual(edge.kind === "exchange" ? points.reverse() : points);
    }
    expect(Object.values(result.positions).every((position) => Number.isFinite(position.x) && Number.isFinite(position.y))).toBe(true);
    expect(result.groups.every((group) => [group.x, group.y, group.width, group.height].every(Number.isFinite))).toBe(true);
    expect(Object.values(result.routes).every((route) => route.every((point) => Number.isFinite(point.x) && Number.isFinite(point.y)))).toBe(true);

    for (let index = 0; index < input.nodes.length; index += 1) {
      const left = nodeBounds(result, input, input.nodes[index].id);
      for (const other of input.nodes.slice(index + 1)) expect(overlaps(left, nodeBounds(result, input, other.id))).toBe(false);
    }
    for (let index = 0; index < result.groups.length; index += 1) {
      for (const other of result.groups.slice(index + 1)) expect(overlaps(result.groups[index], other)).toBe(false);
    }
  }, LARGE_LAYOUT_TIMEOUT_MS);

  it("densifies the 45-node Explain fixture without losing routes or separating fragment bounds", async () => {
    const standardInput = complexInput();
    const standard = await layoutCompound(standardInput);
    const input = { ...standardInput, nodeWidth: 208, density: "dense" as const };
    const result = await layoutCompound(input);
    expect(result.width).toBeLessThan(standard.width);
    expect(Object.keys(result.positions)).toHaveLength(45);
    expect(Object.keys(result.routes)).toHaveLength(44);
    for (let index = 0; index < input.nodes.length; index += 1) {
      const left = nodeBounds(result, input, input.nodes[index].id);
      for (const other of input.nodes.slice(index + 1)) expect(overlaps(left, nodeBounds(result, input, other.id))).toBe(false);
    }
    for (let index = 0; index < result.groups.length; index += 1) {
      for (const other of result.groups.slice(index + 1)) expect(overlaps(result.groups[index], other)).toBe(false);
    }
    for (const edge of input.edges) {
      const route = result.routes[edge.id];
      expect(onBoundary(route[0], nodeBounds(result, input, edge.source))).toBe(true);
      expect(onBoundary(route.at(-1)!, nodeBounds(result, input, edge.target))).toBe(true);
    }
  }, LARGE_LAYOUT_TIMEOUT_MS);

  it("keeps branch and cross-fragment reverse edges in semantic source-to-target route order", async () => {
    const input = complexInput();
    const result = await layoutCompound(input);
    const outgoing = new Map<string, number>();
    for (const edge of input.edges) outgoing.set(edge.source, (outgoing.get(edge.source) ?? 0) + 1);
    expect([...outgoing.values()].some((count) => count > 1)).toBe(true);
    const reversed = input.edges.filter((edge) => edge.reverseForLayout);
    expect(reversed).toHaveLength(9);
    for (const edge of input.edges) {
      const route = result.routes[edge.id];
      expect(route.length).toBeGreaterThanOrEqual(2);
      expect(onBoundary(route[0], nodeBounds(result, input, edge.source))).toBe(true);
      expect(onBoundary(route.at(-1)!, nodeBounds(result, input, edge.target))).toBe(true);
    }
    expect(reversed.every((edge) => onBoundary(result.routes[edge.id][0], nodeBounds(result, input, edge.source)))).toBe(true);
  });

  it("keeps 74px grouped chain cards 22px apart with routes on their visible boundaries", async () => {
    const input: LayoutInput = {
      nodeWidth: 224,
      nodeHeight: 74,
      nodes: [{ id: "source" }, { id: "target" }],
      groups: [{ id: "fragment", nodeIds: ["source", "target"] }],
      edges: [{ id: "source-target", source: "source", target: "target", reverseForLayout: false }],
    };
    const result = await layoutCompound(input);
    const source = nodeBounds(result, input, "source");
    const target = nodeBounds(result, input, "target");
    const route = result.routes["source-target"];

    expect(target.y - (source.y + source.height)).toBeCloseTo(22, 5);
    expect(route[0].y).toBeCloseTo(source.y + source.height, 5);
    expect(route.at(-1)?.y).toBeCloseTo(target.y, 5);

    const dense = await layoutCompound({ ...input, density: "dense" });
    const denseSource = { ...dense.positions.source, width: input.nodeWidth, height: input.nodeHeight };
    const denseTarget = { ...dense.positions.target, width: input.nodeWidth, height: input.nodeHeight };
    expect(denseTarget.y - (denseSource.y + denseSource.height)).toBeCloseTo(18, 5);
    expect(onBoundary(route[0], source)).toBe(true);
    expect(onBoundary(route.at(-1)!, target)).toBe(true);
  });

  it("keeps routes outside unrelated node interiors", async () => {
    const input = complexInput();
    const result = await layoutCompound(input);
    for (const edge of input.edges) {
      for (const node of input.nodes) {
        if (node.id !== edge.source && node.id !== edge.target) {
          expect(crossesRectangleInterior(result.routes[edge.id], nodeBounds(result, input, node.id))).toBe(false);
        }
      }
    }
  });

  it("supports an ungrouped single node", async () => {
    const result = await layoutCompound({ nodes: [{ id: "only" }], edges: [], groups: [], nodeWidth: 224, nodeHeight: 74 });
    expect(result.groups).toEqual([]);
    expect(result.routes).toEqual({});
    expect(result.positions.only).toEqual(expect.objectContaining({ x: expect.any(Number), y: expect.any(Number) }));
  });

  it("rejects duplicate identities, dangling endpoints, ambiguous group membership, and budget overflow", async () => {
    const base: LayoutInput = { nodes: [{ id: "a" }, { id: "b" }], edges: [], groups: [], nodeWidth: 224, nodeHeight: 74 };
    await expect(layoutCompound({ ...base, nodes: [{ id: "a" }, { id: "a" }] })).rejects.toThrow("duplicate node id");
    await expect(layoutCompound({ ...base, edges: [{ id: "dangling", source: "a", target: "missing", reverseForLayout: false }] })).rejects.toThrow("edge endpoint does not exist");
    await expect(layoutCompound({ ...base, groups: [{ id: "first", nodeIds: ["a"] }, { id: "second", nodeIds: ["a"] }] })).rejects.toThrow("unknown or duplicate group member");
    await expect(layoutCompound({ ...base, nodes: Array.from({ length: LAYOUT_BUDGET.nodes + 1 }, (_, index) => ({ id: `n:${index}` })) })).rejects.toThrow("layout budget");
  });
});
