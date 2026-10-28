import type { ExplainAggregatePhase, ExplainCardinalityEstimate, ExplainFactKind, ExplainFactValue, ExplainJoinStrategy, ExplainNodeFact, ExplainNodeFactSet } from "../../../../contracts/generated/doctor-contracts";
import type { LocalizedText } from "../../i18n/locale";

type AvailableFact = Extract<ExplainNodeFact, { state: "available" }>;

export function explainFactLabel(kind: ExplainFactKind | null, text: LocalizedText): string {
  switch (kind) {
    case "cardinality_estimate": return text("Estimated rows", "估算行数");
    case "distribution": return text("Distribution", "分布方式");
    case "join_strategy": return text("Join strategy", "Join 策略");
    case "aggregate_phase": return text("Aggregate phase", "聚合阶段");
    case "plan_child_count": return text("Plan children", "计划子节点");
    default: return text("Fact", "事实");
  }
}

function isCardinalityEstimate(value: ExplainFactValue): value is ExplainCardinalityEstimate {
  return typeof value === "object" && value !== null && value.kind === "integer" && typeof value.value === "string";
}

function displayValue(value: ExplainFactValue, text: LocalizedText): string {
  if (isCardinalityEstimate(value)) return value.value;
  if (typeof value === "number" || typeof value === "string") return String(value);
  return text("Unavailable", "不可用");
}

function enumValue(value: ExplainFactValue, labels: Record<string, [string, string]>, text: LocalizedText): string {
  const label = typeof value === "string" ? labels[value] : undefined;
  return label ? text(...label) : displayValue(value, text);
}

const joinLabels: Record<ExplainJoinStrategy, [string, string]> = {
  inner_broadcast: ["INNER · BROADCAST", "INNER · 广播"],
  inner_partitioned: ["INNER · PARTITIONED", "INNER · 分区"],
  right_outer_bucket_shuffle: ["RIGHT OUTER · BUCKET SHUFFLE", "RIGHT OUTER · BUCKET SHUFFLE"],
  right_outer_partitioned: ["RIGHT OUTER · PARTITIONED", "RIGHT OUTER · 分区"],
};
const aggregateLabels: Record<ExplainAggregatePhase, [string, string]> = {
  update_serialize: ["UPDATE SERIALIZE", "UPDATE SERIALIZE"],
  update_finalize: ["UPDATE FINALIZE", "UPDATE FINALIZE"],
  merge_finalize: ["MERGE FINALIZE", "MERGE FINALIZE"],
};

/** Renders the backend-typed Explain fact verbatim; it never derives a value. */
export function explainFactValue(fact: AvailableFact, text: LocalizedText): string {
  switch (fact.fact_kind) {
    case "cardinality_estimate": return displayValue(fact.value, text);
    case "distribution": return enumValue(fact.value, { broadcast: ["broadcast", "广播"], gather: ["gather", "汇聚"], shuffle: ["shuffle", "Shuffle"] }, text);
    case "join_strategy": return enumValue(fact.value, joinLabels, text);
    case "aggregate_phase": return enumValue(fact.value, aggregateLabels, text);
    case "plan_child_count": return displayValue(fact.value, text);
  }
}

/** The backend-selected card slots, in slot order, as label/value pairs. */
export function explainCardFacts(factSet: ExplainNodeFactSet | undefined, text: LocalizedText): { label: string; value: string }[] {
  if (!factSet) return [];
  const factsByRef = new Map(factSet.facts.map((fact) => [fact.fact_ref, fact] as const));
  return factSet.card_slots.flatMap((slot) => {
    const fact = slot.fact_ref ? factsByRef.get(slot.fact_ref) : undefined;
    if (slot.state !== "selected" || fact?.state !== "available") return [];
    return [{ label: explainFactLabel(fact.fact_kind, text), value: explainFactValue(fact, text) }];
  });
}
