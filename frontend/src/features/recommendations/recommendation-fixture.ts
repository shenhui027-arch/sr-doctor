// Test fixture: a complete ruleset-0.3.0 Recommendation with overridable fields.
import type { Recommendation } from "../../../../contracts/generated/doctor-contracts";

export function recommendationFixture(overrides: Partial<Recommendation> = {}): Recommendation {
  return {
    risk: "r",
    expected_direction: "d",
    validation_steps: [],
    recommendation_id: "rec.test.fixture@v1",
    kind: "investigation",
    title: "Test recommendation",
    next_step: "Test next step",
    risk_level: "low",
    result_effect: "unchanged",
    steps: [],
    expected_changes: [],
    limitations: [],
    ...overrides,
  };
}
