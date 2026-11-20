import { describe, expect, it } from "vitest";
import {
  closeFindingState,
  createSelectionState,
  revealNodeState,
  selectFindingState,
  selectSourceRefState,
  selectSubjectState,
  showFindingNodeState,
  showFindingNodeTransition,
  type Selection,
} from "./selection-store";

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

describe("Doctor drawer close and navigation focus transitions", () => {
  it("restores focus on cancel close, but suppresses focus return when navigating to graph or source", () => {
    const opened = selectFindingState(createSelectionState(), "finding:join", "operator-group:0:4");
    expect(opened).toMatchObject({
      findingId: "finding:join",
      findingOpen: true,
      restoreFindingFocus: true,
    });

    // Cancel close (Esc / close button / overlay) keeps restoreFindingFocus = true.
    const cancelled = closeFindingState(opened);
    expect(cancelled).toMatchObject({
      findingId: "finding:join",
      findingOpen: false,
      restoreFindingFocus: true,
    });

    // Navigating via "在图中定位" closes the drawer with restoreFindingFocus = false and increments revealCount, even on repeat.
    const reopened = selectFindingState(cancelled, "finding:join", "operator-group:0:4");
    const firstLocate = showFindingNodeTransition(reopened, "4");
    expect(firstLocate).toMatchObject({
      findingId: "finding:join",
      nodeId: "4",
      findingOpen: false,
      restoreFindingFocus: false,
      revealCount: 1,
    });
    const secondLocate = showFindingNodeTransition(selectFindingState(firstLocate, "finding:join", "operator-group:0:4"), "4");
    expect(secondLocate.revealCount).toBe(2);
    expect(secondLocate.restoreFindingFocus).toBe(false);

    // Navigating via "查看原文" closes the drawer with restoreFindingFocus = false and increments sourceRevealCount on repeat.
    const firstSource = selectSourceRefState(selectFindingState(secondLocate, "finding:join", "operator-group:0:4"), "text:214");
    expect(firstSource).toMatchObject({
      sourceRef: "text:214",
      findingOpen: false,
      restoreFindingFocus: false,
      sourceRevealCount: 1,
    });
    const secondSource = selectSourceRefState(firstSource, "text:214");
    expect(secondSource.sourceRevealCount).toBe(2);
    expect(secondSource.restoreFindingFocus).toBe(false);

    // Evidence-gaps "在图中定位" uses revealNodeState: increments revealCount and does not restore drawer focus.
    const gapReveal = revealNodeState(opened, "0");
    expect(gapReveal).toMatchObject({
      nodeId: "0",
      findingId: null,
      findingOpen: false,
      restoreFindingFocus: false,
      revealCount: 1,
    });
  });
});
