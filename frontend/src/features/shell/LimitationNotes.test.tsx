import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { LocalizedText } from "../../i18n/locale";
import { LimitationNotes } from "./LimitationNotes";

const english: LocalizedText = (en) => en;
const chinese: LocalizedText = (_en, zh) => zh;

describe("LimitationNotes", () => {
  it("translates known codes, keeps the code only in the title, and folds unknown codes into technical details", () => {
    const html = renderToStaticMarkup(<LimitationNotes codes={["exact_native_digest_is_not_semantic_sql_clustering", "some_future_code"]} text={english} />);
    expect(html).toContain('<li title="exact_native_digest_is_not_semantic_sql_clustering">A query family is an exact StarRocks digest match, not a grouping of similar SQL.</li>');
    expect(html).toContain('<details class="limitation-notes-technical"><summary>Technical details · 1</summary><ul><li><code>some_future_code</code></li></ul></details>');
    expect(renderToStaticMarkup(<LimitationNotes codes={["profile_cache_availability_is_observed_at_fetch_time"]} text={chinese} />)).toContain("Profile 是否仍在缓存中，只有在拉取时才能确定。");
  });

  it("renders nothing for an empty list", () => {
    expect(renderToStaticMarkup(<LimitationNotes codes={[]} text={english} />)).toBe("");
  });
});
