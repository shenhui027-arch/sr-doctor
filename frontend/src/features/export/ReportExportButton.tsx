import { useEffect, useState, type JSX } from "react";
import type { ExportRequest } from "../../../../contracts/generated/doctor-contracts";
import { doctorApi, type ApiFailure, type ReportDownload } from "../../lib/api";
import { useLocale, type LocalizedText } from "../../i18n/locale";

type ExportState =
  | { phase: "idle" }
  | { phase: "exporting" }
  | { phase: "success"; manifestHash: string }
  | { phase: "failure"; messageId: string };

export type ReportExporter = (request: ExportRequest) => Promise<ReportDownload>;
export type ReportSaver = (download: ReportDownload) => void;

export function ReportExportButton({ label, request }: { label: string; request: ExportRequest | null }): JSX.Element {
  const { text } = useLocale();
  const [state, setState] = useState<ExportState>({ phase: "idle" });
  const identity = request?.kind === "analysis" ? request.snapshot.analysis_key : request?.snapshot.comparison_key;
  useEffect(() => setState({ phase: "idle" }), [identity]);

  const exportCurrent = async () => {
    if (!request || state.phase === "exporting") return;
    setState({ phase: "exporting" });
    setState(await runReportExport(request, (value) => doctorApi.exportReport(value), saveReportDownload));
  };

  return <button
    aria-busy={state.phase === "exporting"}
    aria-live="polite"
    className="report-export-button"
    data-phase={state.phase}
    disabled={!request || state.phase === "exporting"}
    onClick={() => { void exportCurrent(); }}
    title={exportTitle(state, text)}
    type="button"
  >{exportLabel(label, state, text)}</button>;
}

export async function runReportExport(request: ExportRequest, exporter: ReportExporter, save: ReportSaver): Promise<ExportState> {
  try {
    const download = await exporter(request);
    save(download);
    return { phase: "success", manifestHash: download.manifestHash };
  } catch (error) {
    return { phase: "failure", messageId: failureMessageId(error) };
  }
}

export function saveReportDownload(download: ReportDownload): void {
  const url = URL.createObjectURL(download.blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = download.filename;
  anchor.rel = "noopener";
  anchor.hidden = true;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
}

function failureMessageId(error: unknown): string {
  if (typeof error === "object" && error !== null && "message_id" in error) return String((error as ApiFailure).message_id);
  return "request_failed";
}

function exportLabel(label: string, state: ExportState, text: LocalizedText): string {
  if (state.phase === "exporting") return text("Exporting…", "正在导出…");
  if (state.phase === "success") return `${text("Exported", "已导出")} · ${state.manifestHash.slice(0, 8)}`;
  if (state.phase === "failure") return `${text("Export failed", "导出失败")} · ${state.messageId}`;
  return label;
}

function exportTitle(state: ExportState, text: LocalizedText): string {
  if (state.phase === "success") return `${text("Downloaded deterministic report", "已下载确定性报告")} · manifest ${state.manifestHash}`;
  if (state.phase === "failure") return `${text("Report download failed", "报告下载失败")} · ${state.messageId}`;
  return text("Download the backend-generated, redacted report", "下载由后端生成并脱敏的报告");
}
