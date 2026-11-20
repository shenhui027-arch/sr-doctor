import { useEffect, useReducer, useRef } from "react";
import type { Dispatch, FormEvent, JSX } from "react";
import type { AnalysisSnapshotReport, RequestedSourceKind } from "../../../../contracts/generated/doctor-contracts";
import { Button, Card, InfoTip, SegTabs, type GridSpan, type SegItem } from "../../design";
import { useLocale, type LocalizedText } from "../../i18n/locale";
import { doctorApi } from "../../lib/api";
import { importFailureMessage } from "./import-errors";
import { hasImportInput, importReducer, initialImportState, type ImportAction, type ImportState } from "./import-state";
import { ProfileInputFields } from "./ProfileInputFields";
import "./import-panel.css";

export type ImportClient = {
  analyzeForm(body: FormData, signal?: AbortSignal): Promise<AnalysisSnapshotReport>;
  analyzeJson(body: unknown, signal?: AbortSignal): Promise<AnalysisSnapshotReport>;
};

export type ImportRequestTracker = {
  start(): AbortController;
  isCurrent(controller: AbortController): boolean;
  finish(controller: AbortController): void;
  abort(): void;
};

/**
 * Tracks the single active offline import request: starting a new request or
 * unmounting aborts the previous `AbortController` so a late superseded reply
 * never overwrites the current view.
 */
export function createImportRequestTracker(): ImportRequestTracker {
  let active: AbortController | null = null;
  return {
    start() {
      active?.abort();
      const next = new AbortController();
      active = next;
      return next;
    },
    isCurrent(controller) {
      return active === controller && !controller.signal.aborted;
    },
    finish(controller) {
      if (active === controller) active = null;
    },
    abort() {
      active?.abort();
      active = null;
    },
  };
}

export async function runTrackedImport(params: {
  api: ImportClient;
  dispatch: Dispatch<ImportAction>;
  onReport(report: AnalysisSnapshotReport): void;
  state: ImportState;
  text: LocalizedText;
  tracker: ImportRequestTracker;
}): Promise<void> {
  const { api, dispatch, onReport, state, text, tracker } = params;
  if (!hasImportInput(state)) {
    dispatch({ type: "fail", message: text("Choose a Profile file or paste a Profile first.", "请先选择或粘贴一个 Profile。") });
    return;
  }
  const controller = tracker.start();
  dispatch({ type: "submit" });
  try {
    let report: AnalysisSnapshotReport;
    if (state.mode === "file" && state.file) {
      const form = new FormData();
      form.append("profile", state.file);
      form.append("source_kind", state.requestedKind);
      report = await api.analyzeForm(form, controller.signal);
    } else {
      report = await api.analyzeJson(
        {
          source_kind: state.requestedKind,
          content: state.pastedText,
        },
        controller.signal,
      );
    }
    if (!tracker.isCurrent(controller)) return;
    dispatch({ type: "complete" });
    onReport(report);
  } catch (error) {
    if (!tracker.isCurrent(controller)) return;
    dispatch({ type: "fail", message: importFailureMessage(error, text) });
  } finally {
    tracker.finish(controller);
  }
}

/** Import card (boards/01-connect.html): drop or paste one Profile, analyzed in this process only. */
export function ImportPanel({
  api = doctorApi,
  idPrefix = "import-profile",
  maxInputBytes = null,
  onCancel,
  onReport,
  span,
}: {
  api?: ImportClient;
  idPrefix?: string;
  maxInputBytes?: number | null;
  onCancel?(): void;
  onReport(report: AnalysisSnapshotReport): void;
  span?: GridSpan;
}): JSX.Element {
  const { text } = useLocale();
  const [state, dispatch] = useReducer(importReducer, initialImportState);
  const trackerRef = useRef<ImportRequestTracker | null>(null);
  if (trackerRef.current === null) trackerRef.current = createImportRequestTracker();
  const formId = `${idPrefix}-form`;

  useEffect(() => () => {
    trackerRef.current?.abort();
  }, []);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runTrackedImport({
      api,
      dispatch,
      onReport,
      state,
      text,
      tracker: trackerRef.current!,
    });
  }

  const kindItems: readonly SegItem<RequestedSourceKind>[] = [
    { value: "auto", label: text("Auto", "自动识别") },
    { value: "text_profile", label: text("Text Profile", "文本 Profile") },
    { value: "json_profile", label: text("JSON Profile", "JSON Profile") },
  ];

  return (
    <Card
      actions={
        <SegTabs
          aria-label={text("Input format", "输入格式")}
          items={kindItems}
          onValueChange={(requestedKind) => dispatch({ type: "choose_kind", requestedKind })}
          size="sm"
          value={state.requestedKind}
        />
      }
      className="import-panel"
      footer={
        <>
          <span className="mono">{text("In-memory offline analysis", "离线内存解析")}</span>
          <div className="import-panel-actions">
            {onCancel ? (
              <Button
                onClick={() => {
                  trackerRef.current?.abort();
                  onCancel();
                }}
                variant="secondary"
              >
                {text("Back to current analysis", "返回当前分析")}
              </Button>
            ) : null}
            <Button
              aria-describedby={`${idPrefix}-status`}
              disabled={state.status === "submitting"}
              form={formId}
              type="submit"
              variant="primary"
            >
              {state.status === "submitting" ? text("Analyzing…", "正在分析…") : text("Analyze Profile →", "分析 Profile →")}
            </Button>
          </div>
        </>
      }
      span={span}
      title={text("Import a Profile", "离线导入 Profile")}
      titleExtra={
        <InfoTip label={text("Supported Profile inputs", "支持的 Profile 来源说明")}>
          {text(
            "Supports StarRocks 3.1+ get_query_profile() text, JSON Profiles and ANALYZE PROFILE output. Analyzed in memory inside this server process only; never written to logs or telemetry.",
            "支持 StarRocks 3.1+ 的 get_query_profile() 文本、JSON Profile 与 ANALYZE PROFILE 输出；原文仅在本服务端进程内解析，不写入日志或遥测。",
          )}
        </InfoTip>
      }
      variant="form"
    >
      <form aria-busy={state.status === "submitting"} className="import-panel-form" id={formId} onSubmit={submit}>
        <p className="card-subtitle import-boundary-note">
          {text(
            "Offline Profile import supported (StarRocks 3.1+ text / JSON / ANALYZE PROFILE). Raw text is sent to the server running SR Doctor, analyzed in-process only, and never written to logs or telemetry.",
            "支持离线导入 StarRocks 3.1+ Profile（文本 / JSON / ANALYZE PROFILE）。原文发送到运行 SR Doctor 的服务端，在服务端进程内分析，不写入日志或遥测。",
          )}
        </p>
        <ProfileInputFields
          dispatch={dispatch}
          idPrefix={idPrefix}
          label="Profile"
          maxInputBytes={maxInputBytes}
          showLegend={false}
          state={state}
        />
        {state.status === "error" && state.error ? (
          <div className="notice-banner notice-banner--danger" role="alert">
            <span>{state.error}</span>
          </div>
        ) : null}
      </form>
      <p aria-live="polite" className="sr-only" id={`${idPrefix}-status`}>
        {state.status === "idle" && text("Choose one Profile file or paste one Profile.", "请选择一个 Profile 文件或粘贴一个 Profile。")}
        {state.status === "ready" && text("Ready to analyze.", "已准备分析。")}
        {state.status === "complete" && text("Analysis complete.", "分析完成。")}
      </p>
    </Card>
  );
}
