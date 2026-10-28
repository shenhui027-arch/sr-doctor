import { describe, expect, it } from "vitest";
import { dataFlowLayout, type DataFlowInput } from "./data-flow-layout";

const card = { width: 176, height: 76 };
// Design board 02: orders ⋈ broadcast(customers) → aggregate → sink, PROJECT unconnected.
const plan: DataFlowInput = {
  nodes: ["0", "1", "2", "4", "5", "6", "7"].map((id) => ({ id, planNodeId: id })),
  edges: [
    { id: "e76", source: "6", target: "7" },
    { id: "e46", source: "4", target: "6" },
    { id: "e04", source: "0", target: "4" },
    { id: "e24", source: "2", target: "4" },
    { id: "e12", source: "1", target: "2" },
  ],
  unconnected: ["5"],
  card,
};

describe("data-flow layout", () => {
  it("reproduces the design board's tree: sources on top, consumers centred over their inputs", () => {
    const layout = dataFlowLayout(plan)!;
    // Shift so the sink column sits where the board draws it (centre x = 300, sources at y = 20).
    const at = (id: string) => ({ x: layout.positions[id].x - layout.rootCenterX + 300, y: layout.positions[id].y + 20 });
    expect(at("0")).toEqual({ x: 40, y: 20 });
    expect(at("1")).toEqual({ x: 384, y: 20 });
    expect(at("2")).toEqual({ x: 384, y: 136 });
    expect(at("4")).toEqual({ x: 212, y: 252 });
    expect(at("6")).toEqual({ x: 212, y: 368 });
    expect(at("7")).toEqual({ x: 212, y: 484 });
    expect(at("5")).toEqual({ x: 424, y: 368 });
  });

  it("routes two inputs into separate ports with staggered bends and draws no edge for unconnected nodes", () => {
    const layout = dataFlowLayout(plan)!;
    const shift = (points: { x: number; y: number }[]) => points.map((point) => ({ x: point.x - layout.rootCenterX + 300, y: point.y + 20 }));
    expect(shift(layout.routes.e04)).toEqual([{ x: 128, y: 96 }, { x: 128, y: 215 }, { x: 261, y: 215 }, { x: 261, y: 252 }]);
    expect(shift(layout.routes.e24)).toEqual([{ x: 472, y: 212 }, { x: 472, y: 233 }, { x: 341, y: 233 }, { x: 341, y: 252 }]);
    expect(shift(layout.routes.e12)).toEqual([{ x: 472, y: 96 }, { x: 472, y: 136 }]);
    expect(Object.keys(layout.routes).sort()).toEqual(["e04", "e12", "e24", "e46", "e76"]);
  });

  it("declines plans that are not a single in-tree, so the caller keeps the general layout", () => {
    expect(dataFlowLayout({ ...plan, edges: [...plan.edges, { id: "multicast", source: "0", target: "2" }] })).toBeNull();
    expect(dataFlowLayout({ ...plan, edges: plan.edges.filter((edge) => edge.id !== "e76") })).toBeNull();
    expect(dataFlowLayout({ ...plan, edges: [...plan.edges, { id: "cycle", source: "7", target: "1" }] })).toBeNull();
  });
});
