import type { ImportState } from "../import/import-state";
import { hasImportInput } from "../import/import-state";

export type ComparisonInputPair = {
  baseline: ImportState;
  candidate: ImportState;
};

export function comparisonInputsReady({ baseline, candidate }: ComparisonInputPair): boolean {
  return hasImportInput(baseline) && hasImportInput(candidate);
}

export function buildComparisonFormData({ baseline, candidate }: ComparisonInputPair): FormData {
  if (!comparisonInputsReady({ baseline, candidate })) {
    throw new Error("Both comparison Profiles are required.");
  }

  const form = new FormData();
  appendProfile(form, "baseline", baseline);
  appendProfile(form, "candidate", candidate);
  return form;
}

function appendProfile(form: FormData, side: "baseline" | "candidate", state: ImportState): void {
  const field = `${side}_profile`;
  if (state.mode === "file" && state.file) {
    form.append(field, state.file);
  } else {
    form.append(field, new Blob([state.pastedText], { type: "text/plain;charset=utf-8" }), `${side}.txt`);
  }
  form.append(`${side}_source_kind`, state.requestedKind);
}
