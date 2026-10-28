import { describe, expect, it } from "vitest";
import { localizedComparisonText } from "./comparison-text";

const chinese = (_english: string, value: string): string => value;

describe("comparison reasons", () => {
  it("explains findings that cannot be called resolved or new (comparator 0.5.0)", () => {
    expect(localizedComparisonText("reason", "finding_not_comparable", chinese)).toBe("部分发现无法判定为已解决或新增：另一侧对同一规则和对象没有完整评估");
  });
});
