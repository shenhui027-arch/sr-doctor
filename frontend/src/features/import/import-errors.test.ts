import { describe, expect, it } from "vitest";
import { importFailureMessage } from "./import-errors";
import { importReducer, initialImportState } from "./import-state";

const en = (english: string) => english;
const zh = (_english: string, chinese: string) => chinese;

describe("importFailureMessage", () => {
  it("turns a structured format mismatch into actionable localized copy", () => {
    const failure = { code: "kind_mismatch", stage: "detect", message_id: "requested_json_mismatch", recoverability: "change_input" };
    expect(importFailureMessage(failure, en)).toContain("does not match");
    expect(importFailureMessage(failure, zh)).toContain("格式与文件内容不匹配");
    expect(importFailureMessage(failure, en)).not.toContain("requested_json_mismatch");
  });

  it("turns budget errors into a bounded-input recovery state", () => {
    const failure = { code: "too_many_nodes", stage: "limits", message_id: "too_many_nodes", recoverability: "reduce_input" };
    const message = importFailureMessage(failure, en);
    const state = importReducer(initialImportState, { type: "fail", message });
    expect(message).toContain("exceeds the local analysis limit");
    expect(importFailureMessage(failure, zh)).toContain("超出本地分析限制");
    expect(state).toMatchObject({ status: "error", error: message });
  });

  it("distinguishes empty and unsupported Profile facts without exposing backend ids", () => {
    const empty = { code: "parse_failed", stage: "detect", message_id: "input_empty", recoverability: "change_input" };
    const unsupported = { code: "parse_failed", stage: "parse", message_id: "no_trusted_facts", recoverability: "change_input" };
    expect(importFailureMessage(empty, en)).toContain("Profile is empty");
    expect(importFailureMessage(unsupported, en)).toContain("No supported StarRocks Profile facts");
    expect(importFailureMessage(unsupported, en)).not.toContain("no_trusted_facts");
  });
});
