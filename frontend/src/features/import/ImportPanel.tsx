import { useReducer } from "react";
import type { FormEvent, JSX } from "react";
import type { AnalysisSnapshotReport } from "../../../../contracts/generated/doctor-contracts";
import { useLocale } from "../../i18n/locale";
import { doctorApi } from "../../lib/api";
import { importFailureMessage } from "./import-errors";
import { hasImportInput, importReducer, initialImportState } from "./import-state";
import { ProfileInputFields } from "./ProfileInputFields";
import "./import-panel.css";

/** Import card (design board 05): drop or paste one Profile, analyzed in this process only. */
export function ImportPanel({ idPrefix = "import-profile", maxInputBytes = null, onCancel, onReport }: {
  idPrefix?: string;
  maxInputBytes?: number | null;
  onCancel?(): void;
  onReport(report: AnalysisSnapshotReport): void;
}): JSX.Element {
  const { text } = useLocale();
  const [state, dispatch] = useReducer(importReducer, initialImportState);
  const hasInput = hasImportInput(state);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!hasInput) {
      dispatch({ type: "fail", message: text("Choose a Profile file or paste a Profile first.", "请先选择或粘贴一个 Profile。") });
      return;
    }
    dispatch({ type: "submit" });
    try {
      if (state.mode === "file" && state.file) {
        const form = new FormData();
        form.append("profile", state.file);
        form.append("source_kind", state.requestedKind);
        onReport(await doctorApi.analyzeForm(form));
      } else {
        onReport(await doctorApi.analyzeJson({
          source_kind: state.requestedKind,
          content: state.pastedText,
        }));
      }
      dispatch({ type: "complete" });
    } catch (error) {
      dispatch({ type: "fail", message: importFailureMessage(error, text) });
    }
  }

  return (
    <section className="import-panel" aria-labelledby={`${idPrefix}-title`}>
      <div className="import-panel-heading">
        <h2 id={`${idPrefix}-title`}>{text("Import a Profile", "导入 Profile")}</h2>
        <span className="chip">{text("Offline · no connection needed", "离线 · 无需连接")}</span>
      </div>
      <form aria-busy={state.status === "submitting"} className="import-panel-form" onSubmit={submit}>
        <ProfileInputFields dispatch={dispatch} idPrefix={idPrefix} label="Profile" maxInputBytes={maxInputBytes} showLegend={false} state={state} />
        {state.status === "error" && <p className="import-panel-error" role="alert">{state.error}</p>}
        <div className="import-panel-footer">
          <span>{text("The Profile is analyzed in this local process only; it is not written to logs or telemetry.", "原文只在本机进程内分析，不写入日志或遥测。")}</span>
          <div className="import-panel-actions">
            {onCancel && <button className="button secondary" onClick={onCancel} type="button">{text("Back to current analysis", "返回当前分析")}</button>}
            <button aria-describedby={`${idPrefix}-status`} className="button accent import-panel-submit" disabled={state.status === "submitting"} type="submit">
              {state.status === "submitting" ? text("Analyzing…", "正在分析…") : text("Analyze Profile", "分析 Profile")}
            </button>
          </div>
        </div>
      </form>
      <p className="visually-hidden" aria-live="polite" id={`${idPrefix}-status`}>
        {state.status === "idle" && text("Choose one Profile file or paste one Profile.", "请选择一个 Profile 文件或粘贴一个 Profile。")}
        {state.status === "ready" && text("Ready to analyze.", "已准备分析。")}
        {state.status === "complete" && text("Analysis complete.", "分析完成。")}
      </p>
    </section>
  );
}
