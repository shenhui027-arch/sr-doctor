import { describe, expect, it } from "vitest";
import { hasImportInput, importByteLength, importReducer, initialImportState } from "./import-state";

describe("import state", () => {
  it("keeps file and paste inputs mutually exclusive while preserving Auto detect", () => {
    const file = new File(["profile"], "sample.json", { type: "application/json" });
    const withFile = importReducer(initialImportState, { type: "choose_file", file });
    const withPaste = importReducer(withFile, { type: "paste", text: "Fragment 0" });

    expect(withFile.status).toBe("ready");
    expect(hasImportInput(withFile)).toBe(true);
    expect(withFile.requestedKind).toBe("auto");
    expect(withPaste.mode).toBe("paste");
    expect(withPaste.file).toBeNull();
    expect(withPaste.status).toBe("ready");
  });

  it("only counts pasted input from memory and resets it on clear", () => {
    const pasted = importReducer(initialImportState, { type: "paste", text: "500ms" });
    const cleared = importReducer(pasted, { type: "clear" });

    expect(importByteLength(pasted)).toBe(5);
    expect(cleared.pastedText).toBe("");
    expect(cleared.status).toBe("idle");
    expect(hasImportInput(cleared)).toBe(false);
  });
});
