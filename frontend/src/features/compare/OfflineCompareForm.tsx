import { useReducer } from "react";
import type { FormEvent, JSX } from "react";
import type { Comparison } from "../../../../contracts/generated/doctor-contracts";
import { useLocale } from "../../i18n/locale";
import { doctorApi } from "../../lib/api";
import { importReducer, initialImportState } from "../import/import-state";
import { ProfileInputFields } from "../import/ProfileInputFields";
import { buildComparisonFormData, comparisonInputsReady } from "./compare-input";
import "./compare.css";

export type ComparisonClient = {
  compareForm(body: FormData): Promise<Comparison>;
};

export type OfflineCompareFormProps = {
  api?: ComparisonClient;
  /** A connection session exists, so the Queries workspace can also pick both executions. */
  connected?: boolean;
  onComparison(comparison: Comparison): void;
};

export function OfflineCompareForm({ api = doctorApi, connected = false, onComparison }: OfflineCompareFormProps): JSX.Element {
  const { text } = useLocale();
  const [baseline, dispatchBaseline] = useReducer(importReducer, initialImportState);
  const [candidate, dispatchCandidate] = useReducer(importReducer, initialImportState);
  const ready = comparisonInputsReady({ baseline, candidate });
  const submitting = baseline.status === "submitting" || candidate.status === "submitting";
  const status = baseline.error ?? candidate.error;

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (!ready || submitting) return;
    dispatchBaseline({ type: "submit" });
    dispatchCandidate({ type: "submit" });

    let comparison: Comparison;
    try {
      comparison = await api.compareForm(buildComparisonFormData({ baseline, candidate }));
    } catch (error) {
      const message = requestFailureMessage(error, text("Comparison request failed.", "对比请求失败。"));
      dispatchBaseline({ type: "fail", message });
      dispatchCandidate({ type: "fail", message });
      return;
    }

    dispatchBaseline({ type: "complete" });
    dispatchCandidate({ type: "complete" });
    onComparison(comparison);
  }

  function clear(): void {
    dispatchBaseline({ type: "clear" });
    dispatchCandidate({ type: "clear" });
  }

  return <section className="offline-compare-form" aria-labelledby="offline-compare-title">
    <header className="offline-compare-head">
      <div>
        <h2 id="offline-compare-title">{text("Compare baseline and candidate Profiles", "对比基线与候选 Profile")}</h2>
        <p>{text(
          "Import the Profiles from before and after your SQL change to compare execution time and resource usage.",
          "导入同一条 SQL 优化前后的 Profile，查看耗时与资源消耗的变化。",
        )}</p>
      </div>
      <span className="chip">{text("Memory only", "仅内存")}</span>
    </header>

    <form aria-busy={submitting} onSubmit={submit}>
      <div className="offline-compare-inputs">
        <ProfileInputFields disabled={submitting} dispatch={dispatchBaseline} idPrefix="compare-baseline" label={text("Baseline Profile", "基线 Profile")} state={baseline} />
        <ProfileInputFields disabled={submitting} dispatch={dispatchCandidate} idPrefix="compare-candidate" label={text("Candidate Profile", "候选 Profile")} state={candidate} />
      </div>
      <div className="offline-compare-actions">
        <button aria-describedby="offline-compare-status" className="button accent" disabled={!ready || submitting} type="submit">
          {submitting ? text("Comparing with local API…", "正在通过本地 API 对比…") : text("Compare locally", "本地对比")}
        </button>
        <button className="button secondary" disabled={submitting} onClick={clear} type="button">{text("Clear both", "清除两项")}</button>
        <p className="offline-compare-status" aria-live="polite" id="offline-compare-status">
          {status ?? (ready
            ? text("Ready to send both Profiles to the same-origin local API.", "已准备将两个 Profile 发送到同源本地 API。")
            : text("Choose or paste both Profiles to continue.", "请选择或粘贴两个 Profile 以继续。"))}
        </p>
      </div>
    </form>

    {connected && <p className="compare-connected-hint">{text(
      "Connected: you can also pick the before and after executions in Queries. Reading existing Profiles does not rerun SQL.",
      "已连接：也可以到“查询”中选择优化前后的两次执行进行对比；读取已有 Profile 不会重新执行 SQL。",
    )}</p>}
  </section>;
}

function requestFailureMessage(error: unknown, fallback: string): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "object" && error !== null && "message_id" in error && typeof error.message_id === "string") {
    return error.message_id;
  }
  return fallback;
}
