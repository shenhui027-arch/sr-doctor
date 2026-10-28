import type {
  AnalysisAttempt,
  AnalysisOutcome,
  ConnectionCreateRequest,
  ConnectionSessionView,
  ErrorSummary,
  ExplainSnapshot,
  MetricCell,
  OptimizationExperimentContext,
  PlanComparison,
  ExportArtifact,
  ExportRequest,
  ExportSubject,
  SubjectHashes,
  FindingDelta,
  HistoryStoreState,
} from './generated/doctor-contracts';

// Readers accept pre-0.6.0 comparisons; current producers always emit the array.
const legacyFindingDelta: FindingDelta = {
  added: [], resolved: [], persisting: [], changed: [], incomparable: [],
  algorithm_changed: [], measurement_changes: [],
};
const partialFindingDelta: FindingDelta = {
  ...legacyFindingDelta, resolved: ['SR3514.TEST:operator-0'],
  partial_evidence: ['SR3514.TEST:operator-0'],
};
// @ts-expect-error Partial evidence contains finding entry strings, not raw evidence.
const invalidPartialFindingDelta: FindingDelta = { ...legacyFindingDelta, partial_evidence: [false] };
const unavailableHistory: HistoryStoreState = {
  state: 'unavailable', reason: 'migration_requires_manual_resolution',
  code: 'history_store_migration_requires_manual_resolution',
};
// TypeScript readers accept old servers; current Rust producers require both fields.
const legacyUnavailableHistory: HistoryStoreState = { state: 'unavailable' };
// @ts-expect-error Internal errors are not public reason categories.
const invalidHistoryReason: HistoryStoreState = { state: 'unavailable', reason: 'private_path', code: 'history_store_open_failed' };

declare const outcome: AnalysisOutcome;
declare const error: ErrorSummary;

const validOutcome: AnalysisAttempt = { outcome };
const validError: AnalysisAttempt = { error };

// @ts-expect-error AnalysisAttempt is an exclusive union.
const invalidBoth: AnalysisAttempt = { outcome, error };
// @ts-expect-error AnalysisAttempt requires one arm.
const invalidNeither: AnalysisAttempt = {};

const exactMetric: MetricCell = {
  metric_cell_ref: 'm1',
  metric_id: 'QuerySpillBytes',
  subject_ref: 'operator-1',
  display_text: '0 B',
  order_rank: 1,
  source_refs: ['json:/metric'],
  reasons: [],
  state: 'exact',
  canonical_number: { kind: 'integer', value: '0', scale: null },
  canonical_unit: 'byte',
  fidelity: 'exact',
  bounds: null,
};

// @ts-expect-error A missing metric cannot carry a fake zero.
const invalidMissingZero: MetricCell = {
  ...exactMetric,
  state: 'missing',
};

// @ts-expect-error Rounded observations require conservative bounds.
const invalidRoundedWithoutBounds: MetricCell = {
  ...exactMetric,
  fidelity: 'rounded',
};

// @ts-expect-error Lossless exact observations cannot carry approximate bounds.
const invalidExactWithBounds: MetricCell = {
  ...exactMetric,
  bounds: {
    lower: { kind: 'integer', value: '0', scale: null },
    upper: { kind: 'integer', value: '1', scale: null },
    lower_inclusive: true,
    upper_inclusive: false,
  },
};

const connectionInput: ConnectionCreateRequest = {
  host: '127.0.0.1',
  port: 9030,
  username: 'workload_user',
  password: 'write-only',
  catalog: null,
  database: null,
  audit_table: null,
  tls_mode: 'docker_loopback_plaintext',
};

declare const connectionView: ConnectionSessionView;
declare const explainSnapshot: ExplainSnapshot;
declare const planComparison: PlanComparison;
declare const experiment: OptimizationExperimentContext;
declare const exportArtifact: ExportArtifact;
declare const exportSubject: ExportSubject;

// @ts-expect-error Response DTOs must never expose request credentials.
connectionView.password;
// @ts-expect-error Plan comparison has no product execution receipt.
planComparison.execution_receipt;
const explainRuntimeUnavailable: 'unavailable' = explainSnapshot.runtime_state;
const externalRerunOnly: 'user_external_rerun' = experiment.candidate_profile_responsibility;
const analysisExportRequest: ExportRequest = { kind: 'analysis', snapshot: validOutcome.outcome.report };
const failedComparisonHashes: SubjectHashes = {
  kind: 'comparison',
  comparison_key: 'comparison:synthetic-export-failed-candidate',
  baseline: {
    state: 'success',
    analysis_key: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
    facts_hash: 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
  },
  candidate: { state: 'failed', error_state: 'parse_failed' },
};

// @ts-expect-error ExportRequest is a strict tagged union.
const invalidExportRequest: ExportRequest = { kind: 'analysis', snapshot: validOutcome.outcome.report, comparison: validOutcome.outcome.report };
const invalidFailedSubjectHash: SubjectHashes = {
  ...failedComparisonHashes,
  // @ts-expect-error A failed subject hash cannot carry a facts hash.
  candidate: { state: 'failed', error_state: 'parse_failed', facts_hash: 'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc' },
};

void [
  validOutcome, validError, invalidBoth, invalidNeither, exactMetric,
  invalidMissingZero, invalidRoundedWithoutBounds, invalidExactWithBounds,
  connectionInput, explainRuntimeUnavailable, externalRerunOnly,
  exportArtifact, exportSubject, analysisExportRequest, failedComparisonHashes,
  invalidExportRequest, invalidFailedSubjectHash,
];
