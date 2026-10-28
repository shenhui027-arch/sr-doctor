import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { sr3514JsonMinimal } from "../../../../contracts/generated/contract-fixtures";
import { sr3514ExportComparisonFailedCandidate } from "../../../../contracts/generated/export-contract-fixtures";
import type { ReportDownload } from "../../lib/api";
import { ReportExportButton, runReportExport } from "./ReportExportButton";

const manifestHash = "a".repeat(64);
const download: ReportDownload = { blob: new Blob(["backend bytes"], { type: "application/json" }), filename: "sr-doctor-report.json", kind: "analysis", manifestHash };

describe("ReportExportButton", () => {
  it("is disabled without a backend report and enabled for an analysis snapshot", () => {
    expect(renderToStaticMarkup(<ReportExportButton label="Export report" request={null} />)).toContain("disabled");
    const html = renderToStaticMarkup(<ReportExportButton label="Export report" request={{ kind: "analysis", snapshot: sr3514JsonMinimal }} />);
    expect(html).toContain("Export report");
    expect(html).not.toContain("disabled");
  });

  it("saves the exact backend Blob and reports its manifest hash", async () => {
    const exporter = vi.fn().mockResolvedValue(download);
    const save = vi.fn();
    const request = { kind: "analysis", snapshot: sr3514JsonMinimal } as const;

    const state = await runReportExport(request, exporter, save);

    expect(exporter).toHaveBeenCalledWith(request);
    expect(save).toHaveBeenCalledWith(download);
    expect(state).toStrictEqual({ phase: "success", manifestHash });
  });

  it("does not save a failed backend response", async () => {
    const save = vi.fn();
    const state = await runReportExport(
      { kind: "analysis", snapshot: sr3514JsonMinimal },
      vi.fn().mockRejectedValue({ message_id: "export_invalid_snapshot" }),
      save,
    );

    expect(save).not.toHaveBeenCalled();
    expect(state).toStrictEqual({ phase: "failure", messageId: "export_invalid_snapshot" });
  });

  it("passes a comparison snapshot through without converting it to analysis", async () => {
    const comparisonDownload: ReportDownload = { ...download, kind: "comparison" };
    const exporter = vi.fn().mockResolvedValue(comparisonDownload);
    const save = vi.fn();
    const request = { kind: "comparison", snapshot: sr3514ExportComparisonFailedCandidate.subject.snapshot } as const;

    await runReportExport(request, exporter, save);

    expect(exporter).toHaveBeenCalledWith(request);
    expect(save).toHaveBeenCalledWith(comparisonDownload);
  });
});
