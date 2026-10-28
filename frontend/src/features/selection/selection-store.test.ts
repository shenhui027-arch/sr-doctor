import { describe, expect, it } from "vitest";
import { selectSubjectState, showFindingNodeState, type Selection } from "./selection-store";

describe("selection state", () => {
  it("clears stale finding and node focus when an exact operator subject is selected", () => {
    const current: Selection = { sourceRef: "text:3", subjectRef: "operator:a", findingId: "finding:a", nodeId: "node:a" };

    expect(selectSubjectState(current, "operator:b")).toEqual({
      sourceRef: "text:3",
      subjectRef: "operator:b",
      findingId: null,
      nodeId: null,
    });
  });
});

describe("showFindingNodeState", () => {
  it("opens the finding's plan node without switching the inspector to another finding", () => {
    const current: Selection = { sourceRef: null, subjectRef: "operator-group:2:4", findingId: "finding:skew", nodeId: null };
    expect(showFindingNodeState(current, "node:4")).toEqual({ ...current, nodeId: "node:4" });
  });
});
