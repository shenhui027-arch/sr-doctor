import { useState } from "react";
import type { ChangeEvent, Dispatch, DragEvent, JSX } from "react";
import { useLocale } from "../../i18n/locale";
import { bytesText } from "../../lib/duration-display";
import {
  importByteLength,
  type ImportAction,
  type ImportState,
} from "./import-state";
import "./profile-input-fields.css";

export type ProfileInputFieldsProps = {
  disabled?: boolean;
  dispatch: Dispatch<ImportAction>;
  idPrefix: string;
  label: string;
  state: ImportState;
  /** The backend's single-input byte budget (capabilities.limits.single_input_bytes), when known. */
  maxInputBytes?: number | null;
  /** The import card already titles itself; the two Compare slots name each input here. */
  showLegend?: boolean;
};

export const PROFILE_FILE_ACCEPT = "application/json,text/plain,.json,.txt,.profile";

type ProfileFileChangeTarget = {
  files: Pick<FileList, "item"> | null;
  value: string;
};

/** Reset the native value after reading so choosing the same file emits change again. */
export function handleProfileFileChange(
  event: Pick<ChangeEvent<HTMLInputElement>, "currentTarget"> | { currentTarget: ProfileFileChangeTarget },
  dispatch: Dispatch<ImportAction>,
): void {
  const input = event.currentTarget;
  dispatch({ type: "choose_file", file: input.files?.item(0) ?? null });
  input.value = "";
}

/**
 * Drop or choose a file, or paste the text (design board 05). Both stay
 * visible; the last one provided is the input, and the backend detects the
 * format, so there is no mode switch or source-kind picker.
 */
export function ProfileInputFields({
  disabled = false,
  dispatch,
  idPrefix,
  label,
  state,
  maxInputBytes = null,
  showLegend = true,
}: ProfileInputFieldsProps): JSX.Element {
  const { text } = useLocale();
  const [dragging, setDragging] = useState(false);
  const limit = maxInputBytes ? bytesText(String(maxInputBytes)) : null;
  const pasteId = `${idPrefix}-paste`;

  function onDrop(event: DragEvent<HTMLLabelElement>) {
    event.preventDefault();
    setDragging(false);
    if (disabled) return;
    dispatch({ type: "choose_file", file: event.dataTransfer.files.item(0) ?? null });
  }

  return <fieldset className="profile-input-fields" disabled={disabled}>
    <legend className={showLegend ? undefined : "visually-hidden"}>{label}</legend>
    <label
      className={`file-drop${dragging ? " dragging" : ""}${state.mode === "file" && state.file ? " chosen" : ""}`}
      onDragLeave={() => setDragging(false)}
      onDragOver={(event) => { event.preventDefault(); setDragging(true); }}
      onDrop={onDrop}
    >
      <svg aria-hidden="true" fill="none" height="32" stroke="currentColor" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.6" viewBox="0 0 24 24" width="32"><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" /><path d="M14 3v5h5M12 18v-6M9 15l3-3 3 3" /></svg>
      <span className="file-drop-title">{state.mode === "file" && state.file ? state.file.name : text("Drop a Profile file here, or click to choose", "拖入 Profile 文件，或点击选择")}</span>
      <span className="file-drop-hint">
        {state.mode === "file" && state.file
          ? `${bytesText(String(importByteLength(state)))} · ${text("held in memory", "仅保留在内存中")}`
          : `${text("ANALYZE PROFILE text and get_query_profile() output", "支持 ANALYZE PROFILE 文本与 get_query_profile() 输出")}${limit ? ` · ${text("up to", "最大")} ${limit}` : ""}`}
      </span>
      <input accept={PROFILE_FILE_ACCEPT} className="visually-hidden" onChange={(event) => handleProfileFileChange(event, dispatch)} type="file" />
    </label>
    <label className="paste-field" htmlFor={pasteId}>
      {text("Or paste the Profile text", "或粘贴 Profile 原文")}
      <textarea
        id={pasteId}
        onChange={(event) => dispatch({ type: "paste", text: event.target.value })}
        placeholder={"Query:\n  Summary:\n     - Query ID: …"}
        rows={4}
        value={state.pastedText}
      />
    </label>
  </fieldset>;
}
