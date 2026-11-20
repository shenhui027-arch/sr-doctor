import { useState } from "react";
import type { ChangeEvent, Dispatch, DragEvent, JSX } from "react";
import { Icon } from "../../design";
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

function nonBreakingBytes(bytes: number): string | null {
  const formatted = bytesText(String(bytes));
  return formatted ? formatted.replace(/\s+/g, "\u00A0") : null;
}

/**
 * Drop or choose a file, or paste the text (boards/01-connect.html). Both stay
 * visible; the last one provided is the input.
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
  const limit = maxInputBytes ? nonBreakingBytes(maxInputBytes) : null;
  const pasteId = `${idPrefix}-paste`;
  const fileId = `${idPrefix}-file`;

  function onDrop(event: DragEvent<HTMLLabelElement>) {
    event.preventDefault();
    setDragging(false);
    if (disabled) return;
    dispatch({ type: "choose_file", file: event.dataTransfer.files.item(0) ?? null });
  }

  return (
    <fieldset className="profile-input-fields" disabled={disabled}>
      <legend className={showLegend ? "field-label" : "sr-only"}>{label}</legend>
      <label
        className={`file-drop${dragging ? " dragging" : ""}${state.mode === "file" && state.file ? " chosen" : ""}`}
        htmlFor={fileId}
        onDragLeave={() => setDragging(false)}
        onDragOver={(event) => { event.preventDefault(); setDragging(true); }}
        onDrop={onDrop}
      >
        <Icon name="upload" size={20} />
        <span className="file-drop-title">
          {state.mode === "file" && state.file
            ? state.file.name
            : text("Drop a Profile file here, or click to choose", "拖入 Profile 文件，或点击选择本地文件")}
        </span>
        <span className="card-subtitle mono file-drop-hint">
          {state.mode === "file" && state.file
            ? `${nonBreakingBytes(importByteLength(state)) ?? "0\u00A0B"} · ${text("in memory", "仅保留在内存中")}`
            : `${limit ? `${text("Up to", "上限")} ${limit} · ` : ""}.txt / .json / .profile`}
        </span>
        <input
          accept={PROFILE_FILE_ACCEPT}
          className="sr-only"
          id={fileId}
          onChange={(event) => handleProfileFileChange(event, dispatch)}
          type="file"
        />
      </label>

      <div className="field paste-field">
        <div className="paste-field-hd">
          <label className="field-label" htmlFor={pasteId}>{text("Raw Profile text", "Profile 原文")}</label>
          <span className="card-subtitle mono">StarRocks 3.1+</span>
        </div>
        <textarea
          className="textarea paste-textarea"
          id={pasteId}
          onChange={(event) => dispatch({ type: "paste", text: event.target.value })}
          placeholder={"Query:\n  Summary:\n     - Query ID: …"}
          rows={5}
          spellCheck={false}
          value={state.pastedText}
        />
      </div>
    </fieldset>
  );
}
