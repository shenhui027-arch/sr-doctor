import { describe, expect, it } from "vitest";
import { importReducer, initialImportState } from "../import/import-state";
import { buildComparisonFormData, comparisonInputsReady } from "./compare-input";

describe("offline comparison input", () => {
  it("builds the backend-owned raw-pair multipart contract without parsing either Profile", async () => {
    const baselineFile = new File(["{\"profile\":\"baseline\"}"], "baseline.json", { type: "application/json" });
    const baseline = importReducer(
      importReducer(initialImportState, { type: "choose_kind", requestedKind: "json_profile" }),
      { type: "choose_file", file: baselineFile },
    );
    const candidate = importReducer(
      importReducer(initialImportState, { type: "choose_mode", mode: "paste" }),
      { type: "paste", text: "Candidate Profile\n  Query ID: candidate-1" },
    );

    const form = buildComparisonFormData({ baseline, candidate });
    const candidatePart = form.get("candidate_profile") as File;

    expect(comparisonInputsReady({ baseline, candidate })).toBe(true);
    expect(Array.from(form.keys())).toStrictEqual([
      "baseline_profile",
      "baseline_source_kind",
      "candidate_profile",
      "candidate_source_kind",
    ]);
    expect(form.get("baseline_profile")).toBe(baselineFile);
    expect(form.get("baseline_source_kind")).toBe("json_profile");
    expect(candidatePart.name).toBe("candidate.txt");
    expect(await candidatePart.text()).toBe("Candidate Profile\n  Query ID: candidate-1");
    expect(form.get("candidate_source_kind")).toBe("auto");
  });

  it("requires both raw Profiles instead of accepting analysis keys", () => {
    const candidate = importReducer(initialImportState, { type: "paste", text: "candidate" });

    expect(comparisonInputsReady({ baseline: initialImportState, candidate })).toBe(false);
    expect(() => buildComparisonFormData({ baseline: initialImportState, candidate })).toThrow("Both comparison Profiles are required.");
  });
});
