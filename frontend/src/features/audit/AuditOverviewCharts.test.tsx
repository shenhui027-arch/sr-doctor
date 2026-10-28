import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { AuditConcurrencyPoint } from "../../../../contracts/generated/doctor-contracts";
import { AuditDigestGuidance, auditSegmentAt } from "./AuditOverviewCharts";

const points: AuditConcurrencyPoint[] = [
  { at_milliseconds: "100000", active_count: "1", x_permyriad: 0, y_permyriad: 2500, label: "00:01:40" },
  { at_milliseconds: "130000", active_count: "4", x_permyriad: 3000, y_permyriad: 10000, label: "00:02:10" },
  { at_milliseconds: "180000", active_count: "1", x_permyriad: 8000, y_permyriad: 2500, label: "00:03:00" },
];

describe("Audit overview helpers", () => {
  it("looks up server segments without recomputing query intervals or rounding decimal time strings", () => {
    expect(auditSegmentAt(points, "152000")).toBe(points[1]);
    expect(auditSegmentAt(points, "180000")).toBe(points[2]);
    expect(auditSegmentAt(points, "99999")).toBeNull();
    const unsafe = [{ ...points[0], at_milliseconds: "9007199254740992", active_count: "9007199254740993" }, { ...points[1], at_milliseconds: "9007199254740993" }];
    expect(auditSegmentAt(unsafe, "9007199254740992")).toBe(unsafe[0]);
    expect(auditSegmentAt(unsafe, "9007199254740993")).toBe(unsafe[1]);
  });

  it("explains enable_sql_digest for an ungroupable window and never offers to change it", () => {
    const html = renderToStaticMarkup(<AuditDigestGuidance text={(english) => english} />);
    expect(html).toContain("Grouping by official digest is unavailable for this period");
    expect(html).toContain("ADMIN SET FRONTEND CONFIG (&quot;enable_sql_digest&quot; = &quot;true&quot;);");
    expect(html).toContain("This tool never changes cluster settings.");
    expect(html).not.toMatch(/<button[^>]*>[^<]*enable_sql_digest/);
  });
});
