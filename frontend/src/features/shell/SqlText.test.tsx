import type { JSX } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { LocaleProvider } from "../../i18n/locale";
import queries from "../queries/QueriesWorkspace.tsx?raw";
import queriesCss from "../queries/queries.css?raw";
import { HeaderStatusText } from "./HeaderStatusText";
import { ExpandableSqlLine, SqlText, SqlToggleButton } from "./SqlText";

const zh = (node: JSX.Element) => renderToStaticMarkup(<LocaleProvider initialLocale="zh">{node}</LocaleProvider>);

describe("SQL reading block", () => {
  it("renders the returned text verbatim in a focusable, labelled pre with a copy control", () => {
    const sql = "SELECT '<b>' AS x\r\nFROM t WHERE name = 'zzzz…'";
    const html = zh(<SqlText copyLabel="复制 SQL" id="sql-1" label="已选审计 SQL" value={sql} />);
    expect(html).toContain('id="sql-1"');
    expect(html).toContain('<pre aria-label="已选审计 SQL" tabindex="0"><code>SELECT &#x27;&lt;b&gt;&#x27; AS x\r\nFROM t');
    expect(html).toContain(">复制 SQL</button>");
    expect(html).toContain('role="status"');
  });

  it("keeps marked-up children but copies the plain value", () => {
    const html = zh(<SqlText copyLabel="复制片段" label="对应 SQL 片段" value="a = 1">a = <mark>1</mark></SqlText>);
    expect(html).toContain("<code>a = <mark>1</mark></code>");
  });
});

describe("SQL show/hide button", () => {
  it("names its block only when expanded and stays focusable while loading", () => {
    const collapsed = zh(<SqlToggleButton controls="sql-1" expanded={false} loading={false} onToggle={() => undefined} />);
    expect(collapsed).toContain('aria-expanded="false"');
    expect(collapsed).not.toContain("aria-controls");
    expect(collapsed).toContain(">查看 SQL<");
    const expanded = zh(<SqlToggleButton controls="sql-1" expanded loading={false} onToggle={() => undefined} />);
    expect(expanded).toContain('aria-controls="sql-1"');
    expect(expanded).toContain(">收起 SQL<");
    const loading = zh(<SqlToggleButton controls="sql-1" expanded={false} loading onToggle={() => undefined} />);
    expect(loading).toContain('aria-busy="true"');
    expect(loading).toContain('aria-disabled="true"');
    expect(loading).not.toMatch(/ disabled=""/);
    expect(loading).toContain(">SQL 加载中…<");
  });

  it("uses the caller's show/hide wording for representative SQL", () => {
    const html = zh(<SqlToggleButton controls="x" expanded hideLabel="收起最慢代表 SQL" loading={false} onToggle={() => undefined} showLabel="查看最慢代表 SQL" />);
    expect(html).toContain(">收起最慢代表 SQL<");
  });
});

describe("one-line SQL with Expand", () => {
  it("shows the line with its full text as title and a collapsed Expand control", () => {
    const html = zh(<ExpandableSqlLine copyLabel="复制模板" id="t-1" label="成员模板" lineClassName="line" truncated value="SELECT id FROM t WHERE id = ?" />);
    expect(html).toContain('<div class="line" title="SELECT id FROM t WHERE id = ?">SELECT id FROM t WHERE id = ?…</div>');
    expect(html).toContain('aria-expanded="false"');
    expect(html).toContain('aria-label="展开 · 成员模板"');
    expect(html).not.toContain("<pre");
  });
});

describe("narrow header status", () => {
  it("keeps the first clause visible and the rest as accessible detail", () => {
    const html = renderToStaticMarkup(<HeaderStatusText value="已连接 · 3.5.14-23a56ec · 只读" />);
    expect(html).toBe('已连接<span class="header-detail"> · 3.5.14-23a56ec · 只读</span>');
    expect(renderToStaticMarkup(<HeaderStatusText value="未连接" />)).toBe("未连接");
  });
});

describe("SQL wiring guards", () => {
  it("Profile list SQL reads below the three columns, never over the query identity", () => {
    expect(queries).toMatch(/<\/div>\n\s*\{\/\*[^\n]*\*\/\}\n\s*\{visibleSql !== null && <SqlText className="query-sql"/);
    expect(queriesCss).not.toMatch(/\.query-sql \{ position: absolute/);
  });
  it("superseding a Profile list request clears the SQL busy state", () => {
    const begin = queries.slice(queries.indexOf("const beginRequest"), queries.indexOf("const requestIsCurrent"));
    expect(begin).toContain("setQuerySqlLoading(false);");
    const slot = queries.slice(queries.indexOf("function setComparisonSlot"), queries.indexOf("async function compareSelectedProfiles"));
    expect(slot).toContain("setQuerySqlLoading(false);");
  });
});
