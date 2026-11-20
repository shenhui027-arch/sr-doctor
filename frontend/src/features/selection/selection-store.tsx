import { createContext, useContext, useMemo, useState } from "react";
import type { JSX, PropsWithChildren } from "react";

export type Selection = {
  sourceRef: string | null;
  subjectRef: string | null;
  findingId: string | null;
  nodeId: string | null;
};

export type SelectionRuntimeState = Selection & {
  findingOpen: boolean;
  /** True when the drawer closed via cancel (Esc, close button, overlay); false when closed by navigating to graph/source. */
  restoreFindingFocus: boolean;
  /** Counts "show in graph" requests, so a view holding the graph behind a tab can bring it forward. */
  revealCount: number;
  /** Counts "view source" requests, so a view holding source behind a tab can bring it forward on repeat clicks. */
  sourceRevealCount: number;
};

export type SelectionStore = SelectionRuntimeState & {
  selectSourceRef(sourceRef: string): void;
  clearSourceRef(): void;
  selectSubject(subjectRef: string | null): void;
  selectFinding(findingId: string, subjectRef: string | null): void;
  closeFinding(): void;
  selectNode(nodeId: string): void;
  /** Selects a plan node from a table and brings the graph forward to show it (v1.0-65 tabs). */
  revealNode(nodeId: string): void;
  /** Opens a finding's plan node without leaving that finding. */
  showFindingNode(nodeId: string): void;
  clearSelection(): void;
};

const SelectionContext = createContext<SelectionStore | null>(null);
const emptySelection: Selection = { sourceRef: null, subjectRef: null, findingId: null, nodeId: null };

export function createSelectionState(initial: Selection = emptySelection): SelectionRuntimeState {
  return {
    ...initial,
    findingOpen: initial.findingId !== null,
    restoreFindingFocus: true,
    revealCount: 0,
    sourceRevealCount: 0,
  };
}

export function selectSubjectState(current: Selection, subjectRef: string | null): Selection {
  return { ...current, subjectRef, nodeId: null, findingId: null };
}

/** A finding's plan node opens beside it: the finding stays selected. */
export function showFindingNodeState(current: Selection, nodeId: string): Selection {
  return { ...current, nodeId };
}

export function selectFindingState(
  current: SelectionRuntimeState,
  findingId: string,
  subjectRef: string | null,
): SelectionRuntimeState {
  return {
    ...current,
    findingId,
    subjectRef,
    nodeId: null,
    findingOpen: true,
    restoreFindingFocus: true,
  };
}

export function closeFindingState(current: SelectionRuntimeState): SelectionRuntimeState {
  return {
    ...current,
    findingOpen: false,
    restoreFindingFocus: true,
  };
}

export function selectSourceRefState(current: SelectionRuntimeState, sourceRef: string): SelectionRuntimeState {
  return {
    ...current,
    sourceRef,
    sourceRevealCount: current.sourceRevealCount + 1,
    findingOpen: false,
    restoreFindingFocus: false,
  };
}

export function revealNodeState(current: SelectionRuntimeState, nodeId: string): SelectionRuntimeState {
  return {
    ...current,
    nodeId,
    subjectRef: null,
    findingId: null,
    revealCount: current.revealCount + 1,
    findingOpen: false,
    restoreFindingFocus: false,
  };
}

export function showFindingNodeTransition(current: SelectionRuntimeState, nodeId: string): SelectionRuntimeState {
  return {
    ...current,
    ...showFindingNodeState(current, nodeId),
    revealCount: current.revealCount + 1,
    findingOpen: false,
    restoreFindingFocus: false,
  };
}

export function SelectionProvider({ children, initialSelection = emptySelection }: PropsWithChildren<{ initialSelection?: Selection }>): JSX.Element {
  const [state, setState] = useState<SelectionRuntimeState>(() => createSelectionState(initialSelection));
  const store = useMemo<SelectionStore>(() => ({
    ...state,
    selectSourceRef: (sourceRef) => setState((current) => selectSourceRefState(current, sourceRef)),
    clearSourceRef: () => setState((current) => ({ ...current, sourceRef: null })),
    selectSubject: (subjectRef) => setState((current) => ({
      ...current,
      ...selectSubjectState(current, subjectRef),
      findingOpen: false,
      restoreFindingFocus: false,
    })),
    selectFinding: (findingId, subjectRef) => setState((current) => selectFindingState(current, findingId, subjectRef)),
    closeFinding: () => setState((current) => closeFindingState(current)),
    selectNode: (nodeId) => setState((current) => ({
      ...current,
      nodeId,
      subjectRef: null,
      findingId: null,
      findingOpen: false,
      restoreFindingFocus: false,
    })),
    revealNode: (nodeId) => setState((current) => revealNodeState(current, nodeId)),
    showFindingNode: (nodeId) => setState((current) => showFindingNodeTransition(current, nodeId)),
    clearSelection: () => setState((current) => ({
      ...current,
      ...emptySelection,
      findingOpen: false,
      restoreFindingFocus: true,
    })),
  }), [state]);

  return <SelectionContext.Provider value={store}>{children}</SelectionContext.Provider>;
}

export function useSelection(): SelectionStore {
  const store = useContext(SelectionContext);
  if (!store) throw new Error("SelectionProvider is required for analysis views.");
  return store;
}
