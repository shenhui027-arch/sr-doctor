import { useEffect, useId, useReducer, useRef } from "react";
import type { Dispatch, FormEvent, JSX } from "react";
import type { Comparison } from "../../../../contracts/generated/doctor-contracts";
import { Button, Card, InfoTip, PageGrid } from "../../design";
import { useLocale } from "../../i18n/locale";
import { doctorApi } from "../../lib/api";
import { importReducer, initialImportState, type ImportAction } from "../import/import-state";
import { ProfileInputFields } from "../import/ProfileInputFields";
import { buildComparisonFormData, comparisonInputsReady, type ComparisonInputPair } from "./compare-input";
import "./compare.css";

export type ComparisonClient = {
  compareForm(body: FormData, signal?: AbortSignal): Promise<Comparison>;
  postForm?<T>(path: string, body: FormData, signal?: AbortSignal): Promise<T>;
};

export type ComparisonRequestTracker = {
  start(): AbortController;
  isCurrent(controller: AbortController): boolean;
  finish(controller: AbortController): void;
  abort(): void;
};

/**
 * Tracks the single active offline comparison request: starting a new request,
 * clearing both slots, or unmounting aborts the previous `AbortController` so
 * a late superseded response never overwrites the current comparison.
 */
export function createComparisonRequestTracker(): ComparisonRequestTracker {
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

function isAbortError(error: unknown): boolean {
  return typeof error === "object" && error !== null && "name" in error && error.name === "AbortError";
}

function extractComparison(value: unknown): Comparison {
  if (typeof value === "object" && value !== null && "comparison" in value) {
    const comparison = (value as { comparison?: unknown }).comparison;
    if (typeof comparison === "object" && comparison !== null) return comparison as Comparison;
  }
  throw new Error("Invalid comparison response.");
}

async function sendComparisonForm(api: ComparisonClient, formData: FormData, signal: AbortSignal): Promise<Comparison> {
  if (api === doctorApi && typeof api.postForm === "function" && api.compareForm.length < 2) {
    return extractComparison(await api.postForm<unknown>("/comparisons", formData, signal));
  }
  return api.compareForm(formData, signal);
}

export async function runTrackedComparison(params: {
  api: ComparisonClient;
  pair: ComparisonInputPair;
  dispatchBaseline: Dispatch<ImportAction>;
  dispatchCandidate: Dispatch<ImportAction>;
  onComparison(comparison: Comparison): void;
  fallbackMessage: string;
  tracker: ComparisonRequestTracker;
}): Promise<void> {
  const { api, pair, dispatchBaseline, dispatchCandidate, onComparison, fallbackMessage, tracker } = params;
  if (!comparisonInputsReady(pair)) return;
  const controller = tracker.start();
  dispatchBaseline({ type: "submit" });
  dispatchCandidate({ type: "submit" });

  try {
    const formData = buildComparisonFormData(pair);
    const comparison = await sendComparisonForm(api, formData, controller.signal);
    if (!tracker.isCurrent(controller)) return;
    dispatchBaseline({ type: "complete" });
    dispatchCandidate({ type: "complete" });
    onComparison(comparison);
  } catch (error) {
    if (!tracker.isCurrent(controller) || isAbortError(error)) return;
    const message = requestFailureMessage(error, fallbackMessage);
    dispatchBaseline({ type: "fail", message });
    dispatchCandidate({ type: "fail", message });
  } finally {
    tracker.finish(controller);
  }
}

export type OfflineCompareFormProps = {
  api?: ComparisonClient;
  /** A connection session exists, so the Queries workspace can also pick both executions. */
  connected?: boolean;
  maxInputBytes?: number | null;
  onComparison(comparison: Comparison): void;
};

export function OfflineCompareForm({
  api = doctorApi,
  connected = false,
  maxInputBytes = null,
  onComparison,
}: OfflineCompareFormProps): JSX.Element {
  const { text } = useLocale();
  const [baseline, dispatchBaseline] = useReducer(importReducer, initialImportState);
  const [candidate, dispatchCandidate] = useReducer(importReducer, initialImportState);
  const trackerRef = useRef<ComparisonRequestTracker | null>(null);
  if (trackerRef.current === null) trackerRef.current = createComparisonRequestTracker();
  const formId = `offline-compare-form-${useId()}`;

  useEffect(() => () => {
    trackerRef.current?.abort();
  }, []);

  const ready = comparisonInputsReady({ baseline, candidate });
  const submitting = baseline.status === "submitting" || candidate.status === "submitting";
  const errorMessage = baseline.error ?? candidate.error;

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (!ready) return;
    await runTrackedComparison({
      api,
      pair: { baseline, candidate },
      dispatchBaseline,
      dispatchCandidate,
      onComparison,
      fallbackMessage: text("Comparison request failed.", "对比请求失败。"),
      tracker: trackerRef.current!,
    });
  }

  function clear(): void {
    trackerRef.current?.abort();
    dispatchBaseline({ type: "clear" });
    dispatchCandidate({ type: "clear" });
  }

  return (
    <div className="workspace-stage cmp-scene">
      <PageGrid className="page-grid--auto">
        <Card
          actions={<span className="badge-stamp">{text("In-memory only", "仅内存解析")}</span>}
          className="offline-compare-card"
          footer={
            <>
              <span aria-live="polite" id="offline-compare-status" role="status">
                {errorMessage ?? (ready
                  ? text("Ready to send both Profiles to the same-origin local API; raw text is never stored on disk.", "已准备将两个 Profile 发送到同源本地 API，原文不落盘。")
                  : text("Choose or paste both Profiles to continue.", "请选择或粘贴两个 Profile 以继续。"))}
              </span>
              <div className="cmp-footer-actions">
                <Button onClick={clear} variant="secondary">
                  {text("Clear both", "清除两项")}
                </Button>
                <Button
                  aria-describedby="offline-compare-status"
                  disabled={!ready || submitting}
                  form={formId}
                  type="submit"
                  variant="primary"
                >
                  {submitting ? text("Comparing with local API…", "正在通过本地 API 对比…") : text("Compare locally", "本地对比")}
                </Button>
              </div>
            </>
          }
          span={{ lg: 12, md: 8, sm: 4 }}
          title={text("Compare baseline and candidate Profiles", "对比基线与候选 Profile")}
          titleExtra={
            <InfoTip label={text("Offline comparison privacy and input boundary", "离线对比隐私与输入边界说明")}>
              {text(
                "Import the Profiles from before and after your SQL change to compare execution time and resource usage. Analyzed in memory inside this process only; never written to SQLite or sent externally.",
                "导入同一条 SQL 优化前后的 Profile，对照实测耗时与资源消耗变化；原文仅在本进程内存中解析，绝不写入 SQLite 历史库或外发。",
              )}
            </InfoTip>
          }
          variant="form"
        >
          <p className="cmp-form-lead">
            {text(
              "Import the Profiles from before and after your SQL change to compare execution time and resource usage.",
              "导入同一条 SQL 优化前后的 Profile，查看耗时与资源消耗的变化。",
            )}
          </p>
          <form aria-busy={submitting} className="cmp-dual-grid" id={formId} onSubmit={submit}>
            <div className="cmp-input-slot">
              <ProfileInputFields
                disabled={submitting}
                dispatch={dispatchBaseline}
                idPrefix="compare-baseline"
                label={text("Baseline Profile", "基线 Profile")}
                maxInputBytes={maxInputBytes}
                state={baseline}
              />
            </div>
            <div className="cmp-input-slot">
              <ProfileInputFields
                disabled={submitting}
                dispatch={dispatchCandidate}
                idPrefix="compare-candidate"
                label={text("Candidate Profile", "候选 Profile")}
                maxInputBytes={maxInputBytes}
                state={candidate}
              />
            </div>
          </form>
          {errorMessage ? (
            <div className="notice-banner notice-banner--danger cmp-form-alert" role="alert">
              <span>{errorMessage}</span>
            </div>
          ) : null}
          {connected ? (
            <div className="notice-banner compare-connected-hint">
              <span>
                {text(
                  "Connected: you can also pick the before and after executions in Queries. Reading existing Profiles does not rerun SQL.",
                  "已连接：也可以到“查询”中选择优化前后的两次执行进行对比；读取已有 Profile 不会重新执行 SQL。",
                )}
              </span>
            </div>
          ) : null}
        </Card>
      </PageGrid>
    </div>
  );
}

function requestFailureMessage(error: unknown, fallback: string): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "object" && error !== null && "message_id" in error && typeof error.message_id === "string") {
    return error.message_id;
  }
  return fallback;
}
