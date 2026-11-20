import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { sr3514JsonMinimal, sr3514TextMinimal } from "../../../../contracts/generated/contract-fixtures";
import type { AnalysisSnapshotReport } from "../../../../contracts/generated/doctor-contracts";
import { SelectionProvider } from "../selection/selection-store";
import { revealSourceTarget, SourcePanel } from "./SourcePanel";

describe("SourcePanel", () => {
  it("renders only safe text lines and source-ref buttons from the analysis DTO", () => {
    const report: AnalysisSnapshotReport = sr3514TextMinimal;
    const html = renderToStaticMarkup(<SelectionProvider><SourcePanel diagnostics={report.diagnostics} provenanceIndex={report.provenance_index} sourceView={report.source_view} /></SelectionProvider>);

    expect(html).toContain("OperatorTotalTime: 500us");
    expect(html).toContain('data-source-ref="text:3"');
    expect(html).toContain("Related source");
    expect(html).toContain('<details class="disclosure technical-disclosure"><summary>Parsing details and reference IDs (advanced)</summary>');
    expect(html).toContain('<details class="disclosure provenance-index"><summary>Full reference index');
    expect(html).not.toContain('<details open');
    expect(html).not.toContain("<script>");
  });

  it("renders backend-provided JSON tokens without reconstructing source text", () => {
    const report: AnalysisSnapshotReport = sr3514JsonMinimal;
    const html = renderToStaticMarkup(<SelectionProvider><SourcePanel diagnostics={report.diagnostics} provenanceIndex={report.provenance_index} sourceView={report.source_view} /></SelectionProvider>);

    expect(html).toContain("500000");
    expect(html).toContain('data-source-ref="json:/operator_total_time_ns"');
  });

  it("escapes malicious safe-view text instead of creating executable DOM", () => {
    const report: AnalysisSnapshotReport = {
      ...sr3514TextMinimal,
      source_view: {
        ...sr3514TextMinimal.source_view,
        plain_text_lines: [{ line_no: 1, source_ref: "text:1", text: '<img src=x onerror="alert(1)"><script>alert(2)</script>' }],
      },
    };
    const html = renderToStaticMarkup(<SelectionProvider><SourcePanel diagnostics={[]} provenanceIndex={["text:1"]} sourceView={report.source_view} /></SelectionProvider>);

    expect(html).toContain("&lt;img");
    expect(html).toContain("&lt;script&gt;");
    expect(html).not.toContain("<img src=");
    expect(html).not.toContain("<script>");
  });

  it("highlights the selected source line and scrolls/focuses the matching row on initial and repeated reveals", () => {
    const html = renderToStaticMarkup(
      <SelectionProvider initialSelection={{ sourceRef: "text:3", subjectRef: null, findingId: null, nodeId: null }}>
        <SourcePanel diagnostics={sr3514TextMinimal.diagnostics} provenanceIndex={sr3514TextMinimal.provenance_index} sourceView={sr3514TextMinimal.source_view} />
      </SelectionProvider>,
    );
    expect(html).toContain('<tr class="is-selected"><td class="source-line-num"><button aria-pressed="true" class="row-pick-btn mono" data-source-ref="text:3" type="button">');

    const rowScroll = vi.fn();
    const otherScroll = vi.fn();
    const targetFocus = vi.fn();
    const row = { scrollIntoView: rowScroll };
    const otherButton = {
      getAttribute: (name: string) => (name === "data-source-ref" ? "text:1" : null),
      closest: () => ({ scrollIntoView: otherScroll }),
      focus: vi.fn(),
    };
    const targetButton = {
      getAttribute: (name: string) => (name === "data-source-ref" ? "text:214" : null),
      closest: (selector: string) => (selector === "tr" ? row : null),
      focus: targetFocus,
    };
    const container = {
      querySelectorAll: () => [otherButton, targetButton],
    } as unknown as ParentNode;

    // First reveal scrolls the matching <tr> to center and focuses its button without scroll jump.
    expect(revealSourceTarget(container, "text:214")).toBe(true);
    expect(rowScroll).toHaveBeenCalledTimes(1);
    expect(rowScroll).toHaveBeenLastCalledWith({ block: "center" });
    expect(targetFocus).toHaveBeenCalledTimes(1);
    expect(targetFocus).toHaveBeenLastCalledWith({ preventScroll: true });
    expect(otherScroll).not.toHaveBeenCalled();

    // Repeated reveal on the same sourceRef (triggered via sourceRevealCount while already on the source tab) scrolls again.
    expect(revealSourceTarget(container, "text:214")).toBe(true);
    expect(rowScroll).toHaveBeenCalledTimes(2);
    expect(targetFocus).toHaveBeenCalledTimes(2);

    // Absent or null references return false without scrolling.
    expect(revealSourceTarget(container, "text:9999")).toBe(false);
    expect(revealSourceTarget(container, null)).toBe(false);
    expect(rowScroll).toHaveBeenCalledTimes(2);
  });
});
