import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { LocaleProvider } from "../../i18n/locale";
import { initialImportState, type ImportAction } from "./import-state";
import { handleProfileFileChange, PROFILE_FILE_ACCEPT, ProfileInputFields } from "./ProfileInputFields";

describe("ProfileInputFields", () => {
  it("lets the native file picker show StarRocks .profile captures", () => {
    const html = renderToStaticMarkup(
      <LocaleProvider initialLocale="zh">
        <ProfileInputFields dispatch={() => undefined} idPrefix="profile" label="Profile" state={initialImportState} />
      </LocaleProvider>,
    );

    expect(html).toContain(`accept="${PROFILE_FILE_ACCEPT}"`);
    expect(html).not.toContain(".text");
  });

  it("clears the native value so the same file can be selected twice", () => {
    const selected = { name: "repeat.profile" } as File;
    const actions: ImportAction[] = [];
    const target = {
      files: { item: () => selected },
      value: "C:\\fakepath\\repeat.profile",
    };
    const dispatch = (action: ImportAction) => { actions.push(action); };

    handleProfileFileChange({ currentTarget: target }, dispatch);
    target.value = "C:\\fakepath\\repeat.profile";
    handleProfileFileChange({ currentTarget: target }, dispatch);

    expect(actions).toEqual([
      { type: "choose_file", file: selected },
      { type: "choose_file", file: selected },
    ]);
    expect(target.value).toBe("");
  });

  it("replaces the selected file and preserves clearing behavior", () => {
    const first = { name: "first.profile" } as File;
    const second = { name: "second.json" } as File;
    const actions: ImportAction[] = [];
    const target = {
      files: { item: () => first },
      value: "C:\\fakepath\\first.profile",
    };
    const dispatch = (action: ImportAction) => { actions.push(action); };

    handleProfileFileChange({ currentTarget: target }, dispatch);
    target.files = { item: () => second };
    target.value = "C:\\fakepath\\second.json";
    handleProfileFileChange({ currentTarget: target }, dispatch);

    expect(actions).toEqual([
      { type: "choose_file", file: first },
      { type: "choose_file", file: second },
    ]);
    expect(target.value).toBe("");
  });

  it("clears a prior selection when the picker returns no file", () => {
    const actions: ImportAction[] = [];
    const target: { files: { item: (index: number) => File | null } | null; value: string } = {
      files: null,
      value: "C:\\fakepath\\prior.profile",
    };

    handleProfileFileChange({ currentTarget: target }, (action) => { actions.push(action); });
    target.files = { item: () => null };
    target.value = "C:\\fakepath\\prior.profile";
    handleProfileFileChange({ currentTarget: target }, (action) => { actions.push(action); });

    expect(actions).toEqual([
      { type: "choose_file", file: null },
      { type: "choose_file", file: null },
    ]);
    expect(target.value).toBe("");
  });
});
