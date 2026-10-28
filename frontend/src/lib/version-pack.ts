import type { VersionContext } from "../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../i18n/locale";

/** Whether the report was read with an audited version pack (the baseline, or the baseline plus a release's deltas). */
export function hasVersionPack(version: VersionContext): boolean {
  return version.pack_match === "exact_pack" || version.pack_match === "baseline_with_deltas";
}

/** The version pack in one short phrase, read from the DTO: the baseline, or the baseline plus a release line's deltas. */
export function versionPackLabel(version: VersionContext, text: LocalizedText): string {
  const deltas = version.selected_pack?.release_deltas;
  if (version.pack_match === "baseline_with_deltas" && deltas) {
    return text(`3.5.14 baseline + ${deltas.line} deltas`, `3.5.14 基线 + ${deltas.line} 差异`);
  }
  return version.pack_match === "exact_pack" ? text("version pack matched", "版本包匹配") : text("no version pack", "无版本包");
}
