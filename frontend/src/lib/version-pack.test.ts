import { describe, expect, it } from "vitest";
import type { VersionContext } from "../../../contracts/generated/doctor-contracts";
import { hasVersionPack, versionPackLabel } from "./version-pack";

const zh = (_en: string, zhText: string) => zhText;
const pack = { pack_id: "starrocks-3.5.14", product_pack_version: "0.1.10", upstream_release: "3.5.14", upstream_commit: "23a56ec", manifest_hash: "h" };
const version = (overrides: Partial<VersionContext>): VersionContext => ({
  reported_version: null, user_declared_version: null, effective_version: "3.5.14", version_source: "detected",
  selected_pack: pack, pack_match: "exact_pack", compatibility: "compatible", support_status: "supported",
  detection_evidence: [], compatibility_checks: [], issues: [], ...overrides,
});

describe("version pack label", () => {
  it("names the baseline and the release line", () => {
    expect(versionPackLabel(version({}), zh)).toBe("版本包匹配");
    const deltas = { release: "3.4.10", line: "3.4", delta_ids: [], deltas_hash: "h" };
    expect(versionPackLabel(version({ pack_match: "baseline_with_deltas", selected_pack: { ...pack, release_deltas: deltas } }), zh)).toBe("3.5.14 基线 + 3.4 差异");
    expect(versionPackLabel(version({ pack_match: "no_pack", selected_pack: null }), zh)).toBe("无版本包");
  });

  it("treats the baseline and baseline-with-deltas as packed", () => {
    expect(hasVersionPack(version({}))).toBe(true);
    expect(hasVersionPack(version({ pack_match: "baseline_with_deltas" }))).toBe(true);
    expect(hasVersionPack(version({ pack_match: "no_pack", selected_pack: null }))).toBe(false);
  });
});
