import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { sr3514ConnectedContractGolden } from "../../../../contracts/generated/connected-contract-fixtures";
import { LocaleProvider } from "../../i18n/locale";
import { RunningProfileEntry } from "./RunningProfileEntry";

function api() {
  return {
    readRunningProfileCandidates: vi.fn(),
    readRunningProfileSnapshot: vi.fn(),
    readOfficialProfile: vi.fn(),
  };
}

describe("RunningProfileEntry SSR", () => {
  it("renders the zero-I/O manual boundary without a SQL control or an automatic read", () => {
    const client = api();
    const html = renderToStaticMarkup(<RunningProfileEntry api={client} session={sr3514ConnectedContractGolden.connection} />);

    expect(html).toContain("Running queries");
    expect(html).toContain("Connected");
    expect(html).not.toContain("DRAFT");
    expect(html).not.toContain("MOCK");
    expect(html).toContain("Read running queries");
    // Hiding this tool's own statements is on by default, as in the Profile list.
    expect(html).toContain('<label class="running-profile-statement-filter"><input type="checkbox" checked=""/>Hide this tool&#x27;s queries</label>');
    expect(html).toContain("Nothing here executes SQL.");
    // The selected-query panel opens only after a query is chosen.
    expect(html).not.toContain("running-profile-context");
    expect(html).toContain('class="running-profile-grid no-selection"');
    expect(html).not.toContain(sr3514ConnectedContractGolden.connection.connection_id);
    expect(html).not.toContain("textarea");
    expect(client.readRunningProfileCandidates).not.toHaveBeenCalled();
    expect(client.readRunningProfileSnapshot).not.toHaveBeenCalled();
    expect(client.readOfficialProfile).not.toHaveBeenCalled();
  });

  it("keeps the Chinese copy plain and hides read actions before candidate selection", () => {
    const client = api();
    const html = renderToStaticMarkup(<LocaleProvider initialLocale="zh"><RunningProfileEntry api={client} session={sr3514ConnectedContractGolden.connection} /></LocaleProvider>);

    expect(html).toContain("运行中的查询");
    expect(html).toContain("读取运行中查询");
    expect(html).toContain("列表不会自动刷新");
    expect(html).not.toContain("读取 Profile 快照");
    expect(html).not.toContain("读取官方解读");
  });
});
