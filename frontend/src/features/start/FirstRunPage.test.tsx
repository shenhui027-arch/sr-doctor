import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { LocaleProvider } from "../../i18n/locale";
import { initialConnectionState } from "../connection/connection-state";
import { FirstRunPage } from "./FirstRunPage";

const render = (locale: "en" | "zh") => renderToStaticMarkup(<LocaleProvider initialLocale={locale}><FirstRunPage connection={initialConnectionState} dispatch={() => undefined} onReport={() => undefined} /></LocaleProvider>);

describe("FirstRunPage", () => {
  it("puts offline import and the read-only connection side by side without inventing cluster data", () => {
    const html = render("en");
    expect(html).toContain("Start diagnosing from a Profile");
    expect(html).toContain("Import a Profile");
    expect(html).toContain("Connect StarRocks");
    expect(html).toContain("Read-only connection");
    // A distinct label: the dialog keeps "Connect read-only" for its own submit.
    expect(html).toContain(">Connect and probe</button>");
    expect(html).not.toContain(">Connect read-only</button>");
    expect(html).not.toMatch(/Demo|Mock cluster/);
    // Its import fields never collide with the Doctor importer's names or ids.
    expect(html).toContain('id="start-profile-paste"');
    expect(html).not.toContain('id="import-profile-paste"');
    // Before a connection the probe rows claim nothing.
    expect(html).toContain("Probed on connect");
  });

  it("connects like a desktop SQL client: any host, plaintext by default, TLS verify-full as a choice", () => {
    const html = render("zh");
    expect(html).toContain("从一个 Profile 开始诊断");
    expect(html).toContain('<input type="radio" name="start-tls" checked="" value="plaintext"/>');
    expect(html).toContain('<input type="radio" name="start-tls" value="verify_full"/>');
    expect(html).toContain("口令与 SQL 不加密传输");
    expect(html).not.toContain("docker_loopback_plaintext");
    expect(html).toContain("凭据只保存在本进程的短期会话中");
  });
});
