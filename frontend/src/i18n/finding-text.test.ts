import { describe, expect, it } from "vitest";
import { localizedFindingText } from "./finding-text";

const chinese = (_english: string, value: string): string => value;

describe("localizedFindingText", () => {
  it("localizes frozen rule and evidence-completeness values", () => {
    expect(localizedFindingText(
      "SR3514.SCAN_TIME_SHARE",
      "risk",
      "scan time share needs manual investigation; it does not prove pruning or I/O root cause",
      chinese,
    )).toBe("扫描时间占比需要人工调查；它不能证明裁剪或 I/O 根因");
    expect(localizedFindingText(
      "SR3514.SCAN_TIME_SHARE",
      "evidence_completeness_reason",
      "exact_typed_inputs",
      chinese,
    )).toBe("输入已类型化且精确");
  });

  it("localizes every ruleset 0.2.0 signal rule from its exact backend copy", () => {
    expect(localizedFindingText("SR3514.OPERATOR_TIME_SKEW", "category", "runtime_skew", chinese)).toBe("运行时倾斜");
    expect(localizedFindingText("SR3514.JOIN_PROBE_FANOUT", "risk", "join fan-out does not prove a wrong join condition", chinese)).toBe("Join 扇出不能证明 Join 条件有误");
    expect(localizedFindingText("SR3514.BROADCAST_EXCHANGE_BYTES", "expected_direction", "reduce broadcast bytes after one controlled manual change", chinese)).toBe("一次受控人工变更后降低广播字节数");
    expect(localizedFindingText("SR3514.STREAMING_PREAGG_INEFFECTIVE", "validation_step", "re-profile after one controlled manual change", chinese)).toBe("一次受控人工变更后重新采集 Profile");
    expect(localizedFindingText("SR3514.NODE_TIME_SHARE", "evidence_completeness_reason", "matching_plan_node_missing", chinese)).toBe("缺少匹配的计划节点");
  });

  it("preserves unknown backend values instead of guessing a translation", () => {
    expect(localizedFindingText(
      "SR3514.FUTURE_RULE",
      "evidence_completeness_reason",
      "future_reason_code",
      chinese,
    )).toBe("future_reason_code");
    expect(localizedFindingText(
      "SR3514.FUTURE_RULE",
      "evidence_completeness_reason",
      "exact_typed_inputs",
      chinese,
    )).toBe("exact_typed_inputs");
  });
});
