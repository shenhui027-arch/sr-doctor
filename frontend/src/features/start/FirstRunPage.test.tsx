import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { LocaleProvider } from "../../i18n/locale";
import { initialConnectionState } from "../connection/connection-state";
import { FirstRunPage } from "./FirstRunPage";

const render = (locale: "en" | "zh") =>
  renderToStaticMarkup(
    <LocaleProvider initialLocale={locale}>
      <FirstRunPage connection={initialConnectionState} dispatch={() => undefined} onReport={() => undefined} />
    </LocaleProvider>,
  );

describe("FirstRunPage (boards/01-connect)", () => {
  it("puts read-only connection and offline import side by side in two card--form cards without intro or step cards", () => {
    const html = render("en");
    expect(html.match(/card--form/g)).toHaveLength(2);
    expect(html).not.toContain("first-run-steps");
    expect(html).not.toContain("Start diagnosing from a Profile");
    expect(html).toContain("Connect StarRocks");
    expect(html).toContain("Import a Profile");
    expect(html).toContain(">Connect and probe</button>");
    expect(html).toContain(">Analyze Profile →</button>");
    expect(html).not.toContain(">Connect read-only</button>");
    expect(html).not.toMatch(/Demo|Mock cluster/);
    // Import fields keep distinct ids from the Doctor importer.
    expect(html).toContain('id="start-profile-paste"');
    expect(html).not.toContain('id="import-profile-paste"');
    // Before a connection the probe rows claim nothing.
    expect(html).toContain("Probed on connect");
  });

  it("connects like a desktop SQL client: any host, plaintext by default with visible warning, TLS verify-full as a choice", () => {
    const html = render("zh");
    expect(html).toContain(">连接并探测能力</button>");
    expect(html).toContain(">分析 Profile →</button>");
    expect(html).toContain('name="start-tls"');
    expect(html).toContain('value="plaintext"');
    expect(html).toContain('value="verify_full"');
    expect(html).toContain("口令与 SQL 不加密传输");
    expect(html).not.toContain("docker_loopback_plaintext");
  });
});
