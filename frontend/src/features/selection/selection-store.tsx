import { createContext, useContext, useMemo, useState } from "react";
import type { JSX, PropsWithChildren } from "react";

export type Selection = {
  sourceRef: string | null;
  subjectRef: string | null;
  findingId: string | null;
  nodeId: string | null;
};

type SelectionStore = Selection & {
  selectSourceRef(sourceRef: string): void;
  clearSourceRef(): void;
  selectSubject(subjectRef: string | null): void;
  selectFinding(findingId: string, subjectRef: string | null): void;
  selectNode(nodeId: string): void;
  /** Opens a finding's plan node without leaving that finding. */
  showFindingNode(nodeId: string): void;
  clearSelection(): void;
};

const SelectionContext = createContext<SelectionStore | null>(null);
const emptySelection: Selection = { sourceRef: null, subjectRef: null, findingId: null, nodeId: null };

export function selectSubjectState(current: Selection, subjectRef: string | null): Selection {
  return { ...current, subjectRef, nodeId: null, findingId: null };
}

/** A finding's plan node opens beside it: the finding stays selected. */
export function showFindingNodeState(current: Selection, nodeId: string): Selection {
  return { ...current, nodeId };
}

export function SelectionProvider({ children, initialSelection = emptySelection }: PropsWithChildren<{ initialSelection?: Selection }>): JSX.Element {
  const [selection, setSelection] = useState<Selection>(() => initialSelection);
  const store = useMemo<SelectionStore>(() => ({
    ...selection,
    selectSourceRef: (sourceRef) => setSelection((current) => ({ ...current, sourceRef })),
    clearSourceRef: () => setSelection((current) => ({ ...current, sourceRef: null })),
    selectSubject: (subjectRef) => setSelection((current) => selectSubjectState(current, subjectRef)),
    selectFinding: (findingId, subjectRef) => setSelection((current) => ({ ...current, findingId, subjectRef, nodeId: null })),
    selectNode: (nodeId) => setSelection((current) => ({ ...current, nodeId, subjectRef: null, findingId: null })),
    showFindingNode: (nodeId) => setSelection((current) => showFindingNodeState(current, nodeId)),
    clearSelection: () => setSelection(emptySelection),
  }), [selection]);

  return <SelectionContext.Provider value={store}>{children}</SelectionContext.Provider>;
}

export function useSelection(): SelectionStore {
  const store = useContext(SelectionContext);
  if (!store) throw new Error("SelectionProvider is required for analysis views.");
  return store;
}
