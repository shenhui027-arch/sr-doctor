import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { sr3514JsonMinimal, sr3514TextMinimal } from "../../../../contracts/generated/contract-fixtures";
import type { AnalysisSnapshotReport } from "../../../../contracts/generated/doctor-contracts";
import { SelectionProvider } from "../selection/selection-store";
import { SourcePanel } from "./SourcePanel";

describe("SourcePanel", () => {
  it("renders only safe text lines and source-ref buttons from the analysis DTO", () => {
    const report: AnalysisSnapshotReport = sr3514TextMinimal;
    const html = renderToStaticMarkup(<SelectionProvider><SourcePanel diagnostics={report.diagnostics} provenanceIndex={report.provenance_index} sourceView={report.source_view} /></SelectionProvider>);

    expect(html).toContain("OperatorTotalTime: 500us");
    expect(html).toContain('data-source-ref="text:3"');
    expect(html).toContain("Related source");
    expect(html).toContain('<details class="technical-disclosure"><summary>Parsing details and reference IDs (advanced)</summary>');
    expect(html).toContain('<details class="provenance-index"><summary>Full reference index');
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
});
