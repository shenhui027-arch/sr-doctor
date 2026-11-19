import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { ConnectionForm, saveUnavailableText } from "../connection/ConnectionForm";
import { initialConnectionState } from "../connection/connection-state";
import { SavedConnectionCard } from "./SavedConnectionCard";

const en = (english: string) => english;
const zh = (_english: string, chinese: string) => chinese;

describe("saved connection (task 72)", () => {
  it("explains every reason saving is off in both languages", () => {
    for (const reason of ["history_unavailable", "key_unavailable", "default_admin_password"] as const) {
      expect(saveUnavailableText(reason, en)).toMatch(/\w/);
      expect(saveUnavailableText(reason, zh)).not.toBe(saveUnavailableText(reason, en));
    }
    expect(saveUnavailableText("default_admin_password", zh)).toContain("DOCTOR_ADMIN_PASSWORD");
  });

  it("offers saving as an unticked choice that stays disabled until the server answers", () => {
    const html = renderToStaticMarkup(createElement(ConnectionForm, { dispatch: () => undefined, idPrefix: "t", state: initialConnectionState }));
    expect(html).toContain("Save the password encrypted on the SR Doctor server and keep collecting");
    // Disabled and unticked: no `checked` attribute on the checkbox itself.
    expect(html).toMatch(/<input disabled="" type="checkbox"\/>/);
  });

  it("starts as a loading card with no forget action before the view arrives", () => {
    const html = renderToStaticMarkup(createElement(SavedConnectionCard));
    expect(html).toContain("Saved connection");
    expect(html).not.toContain("Forget saved connection");
  });
});
