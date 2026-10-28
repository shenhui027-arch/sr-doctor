import type { RequestedSourceKind } from "../../../../contracts/generated/doctor-contracts";

export type { RequestedSourceKind } from "../../../../contracts/generated/doctor-contracts";
export type ImportMode = "file" | "paste";
export type ImportStatus = "idle" | "ready" | "submitting" | "complete" | "error";

export type ImportState = {
  mode: ImportMode;
  requestedKind: RequestedSourceKind;
  file: File | null;
  pastedText: string;
  status: ImportStatus;
  error: string | null;
};

export const initialImportState: ImportState = {
  mode: "file",
  requestedKind: "auto",
  file: null,
  pastedText: "",
  status: "idle",
  error: null,
};

export type ImportAction =
  | { type: "choose_mode"; mode: ImportMode }
  | { type: "choose_kind"; requestedKind: RequestedSourceKind }
  | { type: "choose_file"; file: File | null }
  | { type: "paste"; text: string }
  | { type: "submit" }
  | { type: "complete" }
  | { type: "fail"; message: string }
  | { type: "clear" };

export function hasImportInput(state: ImportState): boolean {
  return state.mode === "file" ? state.file !== null : state.pastedText.trim().length > 0;
}

function ready(state: ImportState): ImportState {
  return { ...state, status: hasImportInput(state) ? "ready" : "idle", error: null };
}

export function importReducer(state: ImportState, action: ImportAction): ImportState {
  switch (action.type) {
    case "choose_mode":
      return ready({ ...state, mode: action.mode });
    case "choose_kind":
      return { ...state, requestedKind: action.requestedKind };
    case "choose_file":
      return ready({ ...state, mode: "file", file: action.file, pastedText: action.file ? "" : state.pastedText });
    case "paste":
      return ready({ ...state, mode: "paste", pastedText: action.text, file: null });
    case "submit":
      return { ...state, status: "submitting", error: null };
    case "complete":
      return { ...state, status: "complete", error: null };
    case "fail":
      return { ...state, status: "error", error: action.message };
    case "clear":
      return { ...initialImportState, requestedKind: state.requestedKind };
  }
}

export function importByteLength(state: ImportState): number {
  return state.mode === "file" ? (state.file?.size ?? 0) : new TextEncoder().encode(state.pastedText).byteLength;
}
