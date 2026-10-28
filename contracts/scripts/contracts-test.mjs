import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..', '..');
const read = (path) => readFile(resolve(root, path), 'utf8');
const readBytes = (path) => readFile(resolve(root, path));
const sha256 = (text) => createHash('sha256').update(text).digest('hex');
const canonical = (value) => {
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  if (value && typeof value === 'object') {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(',')}}`;
  }
  return JSON.stringify(value);
};
const canonicalHash = (value) => sha256(canonical(value));
const without = (value, ...keys) => Object.fromEntries(Object.entries(value).filter(([key]) => !keys.includes(key)));
const byId = (values, id) => values.find((value) => value.case_id === id);
const requireIds = (values, ids, label) => {
  for (const id of ids) if (!byId(values, id)) throw new Error(`${label} missing case: ${id}`);
};

const contractNames = [
  'AnalysisSnapshotReport', 'AnalysisOutcome', 'AnalysisAttempt', 'MetricCell', 'BoundedInterval',
  'GraphView', 'ExecutionSummary', 'ExecutionShape', 'Finding', 'RuleEvaluation', 'Comparison', 'Capabilities', 'ErrorEnvelope',
];
const connectedContractNames = [
  'InputOrigin', 'ConnectionCreateRequest', 'ConnectionCapabilityReport', 'ConnectionSessionView',
  'ConnectedQueryRef', 'ConnectedQueryListRequest', 'ConnectedQueryListResponse', 'ConnectedQuerySqlRequest', 'ConnectedQuerySqlResponse', 'ProfileFetchRequest',
  'ProfileFetchResponse', 'RunningProfileCandidateListRequest', 'RunningProfileCandidateListResponse', 'RunningProfileSnapshotRequest', 'RunningProfileSnapshotResponse', 'ExplainRequest', 'ExplainVersionContext', 'ExplainReceipt', 'ExplainFactKind', 'ExplainFactEvidenceKind', 'ExplainFactValue', 'ExplainDistribution', 'ExplainJoinStrategy', 'ExplainAggregatePhase', 'ExplainCardSlotId', 'ExplainCardFactSlotState', 'ExplainCardFactSlot', 'ExplainNodeFact', 'ExplainNodeFactSet', 'ExplainSourceLocator',
  'ExplainSourceView', 'ExplainSnapshot', 'ExplainResponse', 'ConnectedRecommendation',
  'PlanAdvisorRequest', 'PlanAdvisorResponse', 'CorrelationEvidence', 'PlanComparison',
  'OptimizationExperimentContext', 'ConnectedContractGolden',
];
const exportContractNames = [
  'ExportRequest', 'ExportSubject', 'ExportArtifact', 'ExportManifest', 'ExportFile',
  'SchemaReceipt', 'ReceiptBinding', 'SubjectHashes',
];
const typeText = await read('contracts/generated/doctor-contracts.ts');
const typedFixtureText = await read('contracts/generated/contract-fixtures.ts');
for (const name of contractNames) {
  const schema = JSON.parse(await read(`contracts/schema/${name}.schema.json`));
  if (!schema.definitions?.[name] && schema.title !== name) throw new Error(`${name} schema is missing its root definition`);
  if (!typeText.includes(`export type ${name}`)) throw new Error(`${name} is missing from TypeScript export`);
}
for (const name of connectedContractNames) {
  const schema = JSON.parse(await read(`contracts/schema/${name}.schema.json`));
  if (!schema.definitions?.[name] && schema.title !== name) throw new Error(`${name} schema is missing its root definition`);
  if (!typeText.includes(`export type ${name}`)) throw new Error(`${name} is missing from TypeScript export`);
}
for (const name of exportContractNames) {
  const schema = JSON.parse(await read(`contracts/schema/${name}.schema.json`));
  if (!schema.definitions?.[name] && schema.title !== name) throw new Error(`${name} schema is missing its root definition`);
  if (!typeText.includes(`export type ${name}`)) throw new Error(`${name} is missing from TypeScript export`);
}
const connectedFixtureText = await read('contracts/generated/connected-contract-fixtures.ts');
if (!connectedFixtureText.includes('satisfies ConnectedContractGolden')) {
  throw new Error('connected generated fixture is missing its TypeScript proof');
}
const connectionRequestSchema = JSON.parse(await read('contracts/schema/ConnectionCreateRequest.schema.json'));
const analysisSnapshotSchema = JSON.parse(await read('contracts/schema/AnalysisSnapshotReport.schema.json'));
const explainRequestSchema = JSON.parse(await read('contracts/schema/ExplainRequest.schema.json'));
const explainSnapshotSchema = JSON.parse(await read('contracts/schema/ExplainSnapshot.schema.json'));
const explainLocatorSchema = JSON.parse(await read('contracts/schema/ExplainSourceLocator.schema.json'));
const explainSchemaManifest = JSON.parse(await read('contracts/manifests/doctor-explain-schema-v0.4.json'));
const explainAdapterManifest = JSON.parse(await read('contracts/manifests/sr3514-explain-costs-adapter.json'));
const explainSnapshotSchemaBytes = await readBytes('contracts/schema/ExplainSnapshot.schema.json');
const planComparisonSchema = JSON.parse(await read('contracts/schema/PlanComparison.schema.json'));
const profileFetchSchema = JSON.parse(await read('contracts/schema/ProfileFetchResponse.schema.json'));
const runningCandidateSchema = JSON.parse(await read('contracts/schema/RunningProfileCandidateListResponse.schema.json'));
const runningSnapshotSchema = JSON.parse(await read('contracts/schema/RunningProfileSnapshotResponse.schema.json'));
const connectedQueryRefSchema = JSON.parse(await read('contracts/schema/ConnectedQueryRef.schema.json'));
const connectedGoldenSchema = JSON.parse(await read('contracts/schema/ConnectedContractGolden.schema.json'));
const analysisSnapshotRequired = new Set(analysisSnapshotSchema.required ?? []);
if (analysisSnapshotSchema.properties?.schema_version?.const !== '2.0'
  || !analysisSnapshotRequired.has('graph_view')
  || ['graph_store_v2', 'graph_reading', 'execution_graph_presentation', 'pipeline_timeline',
    'runtime_operator_inventory', 'runtime_occurrence_tops'].some((field) => field in (analysisSnapshotSchema.properties ?? {}))) {
  throw new Error('AnalysisSnapshotReport v2.0 must carry one execution graph and no parallel projections');
}
for (const pointer of [
  connectionRequestSchema.properties?.username?.writeOnly,
  connectionRequestSchema.properties?.password?.writeOnly,
  explainRequestSchema.oneOf?.find((branch) => branch.properties?.input_kind?.enum?.includes('user_sql'))?.properties?.candidate_sql?.writeOnly,
]) {
  if (pointer !== true) throw new Error('connected write-only request field lost its schema marker');
}
if (planComparisonSchema.properties?.schema_version?.const !== '0.1'
  || connectedGoldenSchema.properties?.schema_version?.const !== '0.1'
  || connectedGoldenSchema.definitions?.PlanComparison?.properties?.schema_version?.const !== '0.1') {
  throw new Error('connected schema version is not fixed to 0.1');
}
const explainRequired = new Set(explainSnapshotSchema.required ?? []);
for (const field of ['explain_schema_version', 'explain_level', 'facts_hash', 'version', 'receipt', 'plan_node_fact_sets', 'source_locators', 'source_view', 'diagnostics']) {
  if (!explainRequired.has(field)) throw new Error(`ExplainSnapshot v0.4 field is optional: ${field}`);
}
if (explainSnapshotSchema.properties?.explain_schema_version?.$ref !== '#/definitions/ExplainSchemaVersion'
  || canonical(explainSnapshotSchema.definitions?.ExplainSchemaVersion?.enum) !== canonical(['0.4'])) {
  throw new Error('ExplainSnapshot does not freeze its independent 0.4 schema version');
}
const explainRowLine = explainLocatorSchema.oneOf?.find((variant) => variant.properties?.kind?.enum?.[0] === 'row_line');
for (const field of ['row_no', 'row_line_no', 'global_line_no']) {
  if (explainRowLine?.properties?.[field]?.minimum !== 1) {
    throw new Error(`Explain row_line ${field} must be one-based in the schema`);
  }
}
if (explainSchemaManifest.id !== 'doctor-explain-schema'
  || explainSchemaManifest.version !== '0.4'
  || explainSchemaManifest.explain_schema_version !== '0.4'
  || explainSchemaManifest.schema_id !== 'ExplainSnapshot'
  || explainSchemaManifest.schema_content_sha256 !== sha256(explainSnapshotSchemaBytes)
  || !/^[a-f0-9]{64}$/.test(explainSchemaManifest.schema_definitions_canonical_sha256 ?? '')) {
  throw new Error('Explain receipt schema manifest is not bound to the complete generated ExplainSnapshot schema');
}
const profileFetchRequired = new Set(profileFetchSchema.required ?? []);
for (const field of ['source_hash', 'analysis_key', 'analysis_receipt', 'outcome']) {
  if (!profileFetchRequired.has(field)) throw new Error(`ProfileFetchResponse success evidence is optional: ${field}`);
}
const profileFetchOutcomeRef = profileFetchSchema.properties?.outcome?.$ref
  ?? profileFetchSchema.properties?.outcome?.allOf?.[0]?.$ref;
if (profileFetchSchema.properties?.source_hash?.type !== 'string'
  || profileFetchSchema.properties?.analysis_key?.type !== 'string'
  || profileFetchSchema.properties?.analysis_receipt?.$ref !== '#/definitions/AnalysisReceipt'
  || profileFetchOutcomeRef !== '#/definitions/AnalysisOutcome'
  || canonical(profileFetchSchema.definitions?.ProfileFetchState?.enum) !== canonical(['fetched'])) {
  throw new Error('ProfileFetchResponse no longer models a strict fetched-only success payload');
}
for (const field of ['limit', 'returned', 'at_limit', 'rejected', 'read_at_utc', 'candidates']) {
  if (!runningCandidateSchema.required?.includes(field)) {
    throw new Error(`Running candidate response has optional coverage field: ${field}`);
  }
}
const isNullable = (schema) => schema?.type === 'null'
  || (Array.isArray(schema?.type) && schema.type.includes('null'))
  || schema?.anyOf?.some((branch) => branch.type === 'null');
if (!isNullable(runningCandidateSchema.properties?.profile_observed_at_utc)) {
  throw new Error('Running candidate source observation time must remain explicit unknown/null');
}
for (const field of ['query', 'list_observed_state', 'profile_lifecycle', 'read_at_utc', 'outcome']) {
  if (!runningSnapshotSchema.required?.includes(field)) {
    throw new Error(`Running snapshot response has optional lifecycle field: ${field}`);
  }
}
const runningLifecycleRef = runningSnapshotSchema.properties?.profile_lifecycle?.$ref
  ?? runningSnapshotSchema.properties?.profile_lifecycle?.allOf?.[0]?.$ref;
if (runningLifecycleRef !== '#/definitions/QueryState') {
    throw new Error('Running snapshot lifecycle must be the parser-owned QueryState');
}
if (!isNullable(runningSnapshotSchema.properties?.profile_observed_at_utc)) {
  throw new Error('Running snapshot source observation time must remain explicit unknown/null');
}
const profileActionBranches = connectedQueryRefSchema.definitions?.ConnectedProfileFetchAction?.oneOf ?? [];
const planActionBranches = connectedQueryRefSchema.definitions?.ConnectedPlanCheckAction?.oneOf ?? [];
const actionBranch = (branches, state) => branches.find((branch) => branch.properties?.state?.enum?.includes(state));
const profileAvailable = actionBranch(profileActionBranches, 'available');
const profileUnavailable = actionBranch(profileActionBranches, 'unavailable');
const planAvailable = actionBranch(planActionBranches, 'available');
const planUnavailable = actionBranch(planActionBranches, 'unavailable');
const requiresActionCore = (branch) => ['state', 'reason'].every((field) => branch?.required?.includes(field));
if (profileActionBranches.length !== 2
  || !requiresActionCore(profileAvailable)
  || !requiresActionCore(profileUnavailable)
  || profileAvailable?.properties?.reason?.$ref !== '#/definitions/ProfileFetchAvailableReason'
  || profileUnavailable?.properties?.reason?.$ref !== '#/definitions/ProfileFetchUnavailableReason'
  || profileAvailable?.additionalProperties !== false
  || profileUnavailable?.additionalProperties !== false
  || planActionBranches.length !== 2
  || !requiresActionCore(planAvailable)
  || !requiresActionCore(planUnavailable)
  || planAvailable?.properties?.reason?.$ref !== '#/definitions/PlanCheckAvailableReason'
  || planUnavailable?.properties?.reason?.$ref !== '#/definitions/PlanCheckUnavailableReason'
  || !planAvailable?.required?.includes('plan_sql_ref')
  || planAvailable?.properties?.plan_sql_ref?.type !== 'string'
  || planUnavailable?.properties?.plan_sql_ref !== undefined
  || planAvailable?.additionalProperties !== false
  || planUnavailable?.additionalProperties !== false) {
  throw new Error('connected action state/reason/reference negative matrix is no longer strict');
}
const connectedManifestPath = 'fixtures/manifests/sr3514-connected-contract-fixtures.json';
const connectedManifest = JSON.parse(await read(connectedManifestPath));
const connectedFixtureBytes = await readBytes(connectedManifest.fixture_path);
const connectedGolden = JSON.parse(connectedFixtureBytes.toString('utf8'));
if (connectedManifest.fixture_id !== 'sr3514_connected_contract_golden'
  || connectedManifest.evidence !== 'synthetic_contract_only_not_docker_e2e'
  || connectedManifest.fixture_sha256 !== sha256(connectedFixtureBytes)
  || connectedManifest.manifest_hash !== canonicalHash(without(connectedManifest, 'manifest_hash'))) {
  throw new Error('connected fixture manifest is not deterministic synthetic contract evidence');
}
const connectedSerialized = canonical(connectedGolden);
for (const forbidden of ['"password"', '"username"', '"candidate_sql"', '"execution_receipt"', '"profile_text"']) {
  if (connectedSerialized.includes(forbidden)) throw new Error(`connected golden leaks forbidden ${forbidden}`);
}
if (connectedGolden.schema_version !== '0.1'
  || connectedGolden.evidence !== 'synthetic_contract_only_not_docker_e2e'
  || connectedGolden.explain_response?.snapshot?.explain_schema_version !== '0.4'
  || connectedGolden.explain_response?.snapshot?.explain_level !== 'costs'
  || connectedGolden.explain_response?.snapshot?.graph_view?.topology_source !== 'explain_costs'
  || connectedGolden.explain_response?.snapshot?.runtime_state !== 'unavailable'
  || connectedGolden.plan_comparison?.runtime_delta_state !== 'not_comparable_missing_runtime_profile'
  || connectedGolden.experiment?.candidate_profile_responsibility !== 'user_external_rerun'
  || connectedGolden.experiment?.candidate_profile_state !== 'awaiting_user_external_rerun'
  || connectedGolden.profile_fetch_response?.source_kind !== 'text_profile'
  || connectedGolden.plan_comparison?.correlation?.baseline_source_kind !== 'text_profile'
  || connectedGolden.profile_fetch_response?.source_hash !== '3aeb9a126d70be2f999f4bca2929cb7207542b9f6d1e2cd21f51f163e7db6c9b'
  || !/^[a-f0-9]{64}$/.test(connectedGolden.profile_fetch_response?.analysis_key ?? '')) {
  throw new Error('connected golden lost its plan-only or external-rerun boundary');
}
const explainSnapshot = connectedGolden.explain_response?.snapshot;
const explainRefs = new Set((explainSnapshot?.source_locators ?? []).map((locator) => locator.source_ref));
const requiredExplainRefs = [
  ...(explainSnapshot?.source_refs ?? []),
  ...(explainSnapshot?.version?.source_refs ?? []),
  ...(explainSnapshot?.graph_view?.source_refs ?? []),
];
if (!explainSnapshot?.receipt?.schema?.manifest_hash
  || !explainSnapshot?.receipt?.adapter?.manifest_hash
  || !explainSnapshot?.receipt?.exchange_recipe?.manifest_hash
  || !requiredExplainRefs.every((sourceRef) => explainRefs.has(sourceRef))) {
  throw new Error('Explain fixture lacks receipt evidence or closed source locators');
}
if (explainSnapshot.receipt.adapter.id !== explainAdapterManifest.id
  || explainSnapshot.receipt.adapter.version !== explainAdapterManifest.version
  || explainSnapshot.receipt.adapter.manifest_hash !== canonicalHash(explainAdapterManifest)) {
  throw new Error('Explain fixture adapter receipt drifted from its registered manifest');
}
if (!connectedGolden.profile_fetch_response?.source_hash
  || !connectedGolden.profile_fetch_response?.analysis_receipt?.components?.length
  || !connectedGolden.profile_fetch_response?.outcome?.report
  || !connectedGolden.plan_comparison?.correlation?.candidate_plan_hash) {
  throw new Error('connected golden lacks required source, receipt, or hash evidence');
}
const auditQuery = connectedGolden.query_response?.queries?.[0];
if (connectedGolden.query_response?.source !== 'audit'
  || auditQuery?.discovery_source !== 'audit'
  || auditQuery?.observed_at_utc !== '2026-08-30T00:00:00Z'
  || auditQuery?.timestamp_state !== 'reader_session_interpreted'
  || connectedGolden.connection?.time_zone?.effective_time_zone !== 'Etc/UTC'
  || connectedGolden.connection?.capabilities?.time_zone !== 'available'
  || !connectedGolden.query_response?.limitations?.includes('audit_timestamp_utc_is_reader_session_interpretation_not_plugin_epoch')) {
  throw new Error('audit browse lost server-session time-zone or epoch evidence');
}
const connectedReport = connectedGolden.profile_fetch_response.outcome.report;
const fetchedQuery = connectedGolden.profile_fetch_response.query;
if (fetchedQuery.status !== auditQuery.status
  || fetchedQuery.discovery_source !== auditQuery.discovery_source
  || fetchedQuery.discovery_generation !== connectedGolden.query_response.discovery_generation
  || fetchedQuery.digest !== auditQuery.digest
  || !fetchedQuery.source_refs.includes('profile:query_id')) {
  throw new Error('connected fetch must preserve discovery identity and add profile evidence');
}
if (connectedGolden.profile_fetch_response.state !== 'fetched'
  || connectedGolden.profile_fetch_response.source_hash !== connectedReport.input?.source_hash
  || connectedGolden.profile_fetch_response.analysis_key !== connectedReport.analysis_key
  || canonical(connectedGolden.profile_fetch_response.analysis_receipt) !== canonical(connectedReport.analysis_receipt)
  || connectedGolden.profile_fetch_response.source_kind !== connectedReport.input?.detected_kind) {
  throw new Error('connected fetched outcome summary diverges from its analysis report');
}
for (const name of ['MetricCellState', 'FindingConfidence', 'VersionEvidence', 'StructureDelta', 'FindingDelta', 'TopologySource', 'EdgeSource']) {
  if (!typeText.includes(`export type ${name}`)) throw new Error(`${name} is missing from TypeScript export`);
}
const findingSchema = JSON.parse(await read('contracts/schema/Finding.schema.json'));
// These are wire-boundary checks, not substitutes for connector zero-I/O tests.
const officialRequestSchema = JSON.parse(await read('contracts/schema/OfficialProfileReadRequest.schema.json'));
const officialResponseSchema = JSON.parse(await read('contracts/schema/OfficialProfileReadResponse.schema.json'));
if (!officialRequestSchema.required?.includes('identity')
  || !officialResponseSchema.required?.includes('identity')
  || officialRequestSchema.additionalProperties !== false
  || 'discovery_generation' in (officialRequestSchema.properties ?? {})) {
  throw new Error('official Profile wire contract must require one explicit identity, not legacy top-level generation');
}
const officialIdentitySchema = JSON.parse(await read('contracts/schema/OfficialProfileIdentity.schema.json'));
const identityArms = officialIdentitySchema.oneOf;
if (!Array.isArray(identityArms) || identityArms.length !== 2) {
  throw new Error('official Profile identity must have exactly two mutually exclusive arms');
}
for (const [kind, field] of [['discovery', 'discovery_generation'], ['verified_standard_attempt', 'execution_attempt_id']]) {
  const arm = identityArms.find((item) => canonical(item.properties?.kind?.enum) === canonical([kind]));
  if (!arm || arm.additionalProperties !== false
    || canonical([...arm.required].sort()) !== canonical(['kind', field].sort())
    || canonical(Object.keys(arm.properties).sort()) !== canonical(['kind', field].sort())) {
    throw new Error(`official Profile identity arm ${kind} admits missing or mixed authority`);
  }
}
const eligibilityMethodSchema = JSON.parse(await read('contracts/schema/OfficialProfileEligibilityMethod.schema.json'));
if (canonical(eligibilityMethodSchema.enum) !== canonical(['standard_profile'])) {
  throw new Error('official Profile execution eligibility must exclude native execution');
}
for (const name of ['OfficialProfileIdentity', 'OfficialProfileEligibility', 'OfficialProfileEligibilityMethod']) {
  if (!typeText.includes(`export type ${name} =`)) throw new Error(`${name} is missing from TypeScript export`);
}
if (connectedGolden.profile_fetch_response.official_profile_eligibility !== undefined) {
  throw new Error('discovered Profile fixture must not claim standard execution eligibility');
}
if (findingSchema.properties?.confidence?.$ref !== '#/definitions/FindingConfidence'
  || canonical(findingSchema.definitions?.FindingConfidence?.enum) !== canonical(['high', 'medium', 'low'])
  || !typeText.includes('export type FindingConfidence = "high" | "medium" | "low";')
  || !typeText.includes('confidence: FindingConfidence,')) {
  throw new Error('Finding confidence is not a closed high/medium/low Rust/Schema/TypeScript enum');
}
const attemptSchema = JSON.parse(await read('contracts/schema/AnalysisAttempt.schema.json'));
if (attemptSchema.anyOf?.length !== 2
  || attemptSchema.definitions?.AnalysisOutcomeAttempt?.additionalProperties !== false
  || attemptSchema.definitions?.AnalysisErrorAttempt?.additionalProperties !== false) {
  throw new Error('AnalysisAttempt schema does not enforce outcome/error XOR arms');
}
const comparisonSchema = JSON.parse(await read('contracts/schema/Comparison.schema.json'));
const comparisonReceiptItems = comparisonSchema.definitions?.ComparisonReceipt?.properties?.components;
const receiptComponentProperties = comparisonSchema.definitions?.ReceiptComponent?.properties;
const metricDeltaStates = comparisonSchema.definitions?.MetricDeltaState?.enum;
if (comparisonSchema.properties?.schema_version?.type !== 'string'
  || comparisonSchema.properties?.schema_version?.const !== '2.0'
  || comparisonReceiptItems?.minItems !== 1 || comparisonReceiptItems?.maxItems !== 1
  || receiptComponentProperties?.id?.minLength !== 1
  || receiptComponentProperties?.version?.minLength !== 1
  || receiptComponentProperties?.manifest_hash?.pattern !== '^[0-9A-Fa-f]{64}$'
  || canonical(metricDeltaStates) !== canonical(['comparable', 'not_comparable'])) {
  throw new Error('Comparison schema does not mirror its frozen 2.0 receipt contract');
}
if (typeText.includes('bigint')) throw new Error('wire DTO contains bigint, which is not JSON round-trip compatible');
for (const fixture of ['sr3514JsonMinimal', 'sr3514TextMinimal']) {
  if (!typedFixtureText.includes(`export const ${fixture}`)) throw new Error(`typed fixture missing: ${fixture}`);
}
if ((typedFixtureText.match(/satisfies AnalysisSnapshotReport/g) ?? []).length !== 2) {
  throw new Error('both generated report literals must be checked by TypeScript');
}

const executionGraphManifest = JSON.parse(await read('fixtures/manifests/sr3514-execution-graph.json'));
const executionGraphBytes = await readBytes(executionGraphManifest.report_path);
const executionGraphReport = JSON.parse(executionGraphBytes.toString('utf8'));
if (executionGraphManifest.evidence !== 'offline_production_replay_of_retained_docker_capture_not_new_live_e2e'
  || executionGraphManifest.source_sha256 !== sha256(await readBytes(executionGraphManifest.source_path))
  || executionGraphManifest.report_sha256 !== sha256(executionGraphBytes)
  || executionGraphReport.input.source_hash !== executionGraphManifest.source_sha256
  || !(await read('contracts/generated/execution-graph-fixture.ts')).includes('satisfies AnalysisSnapshotReport')) {
  throw new Error('execution graph production replay has stale source/report hashes or lacks TypeScript proof');
}
const executionSummary = executionGraphReport.execution_summary;
if (executionSummary?.state !== 'available'
  || executionSummary.recipe_id !== 'sr3514.explain_analyzer.node_info.v1'
  || executionSummary.nodes.length !== executionGraphReport.graph_view.nodes.length
  || executionSummary.nodes.some((node) => node.total_time.state !== 'exact' || node.reasons.length)
  || executionSummary.nodes.some((node) => node.total_time.state === 'unavailable' && node.total_time.canonical_number !== null)
  || executionGraphReport.analysis_receipt.components.length !== 6) {
  throw new Error('execution graph fixture lost its exact ExplainAnalyzer node summary');
}

const fixtureManifestPath = 'fixtures/manifests/sr3514-contract-fixtures.json';
const fixtureManifest = JSON.parse(await read(fixtureManifestPath));
const comparatorManifestPath = fixtureManifest.comparison_component_manifest?.path;
if (comparatorManifestPath !== 'contracts/manifests/doctor-comparator-v0.1.json') {
  throw new Error('fixture manifest comparator path mismatch');
}
const comparatorManifest = JSON.parse(await read(comparatorManifestPath));
const comparatorSemantic = canonicalHash(without(
  comparatorManifest,
  'evidence', 'status', 'golden_case_ids', 'manifest_hash', 'semantic_manifest_hash',
));
if (comparatorManifest.semantic_manifest_hash !== comparatorSemantic
  || comparatorManifest.manifest_hash !== canonicalHash(without(comparatorManifest, 'manifest_hash'))
  || fixtureManifest.comparison_component_manifest.sha256 !== comparatorManifest.manifest_hash) {
  throw new Error('comparator manifest or fixture link hash mismatch');
}
if (comparatorManifest.id !== 'doctor-comparator-v0.1' || comparatorManifest.version !== '0.6.0'
  || comparatorManifest.analysis_schema_range !== '2.0' || comparatorManifest.comparison_schema_range !== '2.0'
  || comparatorManifest.status !== 'contract_only_not_ready') {
  throw new Error('comparator manifest identity or status mismatch');
}
for (const key of ['attempt_policy', 'metric_policy', 'structure_policy', 'finding_policy', 'deterministic_order_policy', 'registry_equivalence']) {
  if (!comparatorManifest[key] || typeof comparatorManifest[key] !== 'object') {
    throw new Error(`comparator manifest missing ${key}`);
  }
}
const frozenMetricPolicy = {
  absolute_delta: {
    exact_formula: 'candidate_minus_baseline',
    interval: {
      lower_formula: 'candidate.lower_minus_baseline.upper',
      lower_inclusive: 'candidate.lower_inclusive_and_baseline.upper_inclusive',
      upper_formula: 'candidate.upper_minus_baseline.lower',
      upper_inclusive: 'candidate.upper_inclusive_and_baseline.lower_inclusive',
    },
  },
  duplicate_match: 'not_comparable_with_limitation',
  match_key: ['metric_id', 'subject_match'],
  group_refinement: 'pair_by_cell_match_key_when_every_cell_on_both_sides_has_a_unique_key',
  percentage: {
    arithmetic: 'checked_i128_rational_no_f64',
    baseline_closure_contains_zero: 'undefined_baseline_interval_contains_zero',
    exact_formula: 'absolute_delta_divided_by_baseline',
    fixed_decimal_scale: 9,
    interval_requires: 'baseline_closure_strictly_positive',
    lower_endpoint: 'candidate.lower_div_baseline.upper_minus_one',
    interval_lower_rounding: 'floor',
    endpoint_inclusivity: {
      exact: 'corresponding_input_endpoint_conjunction',
      outward_rounding: 'conservative_inclusive',
    },
    interval_upper_rounding: 'ceil',
    upper_endpoint: 'candidate.upper_div_baseline.lower_minus_one',
  },
  typed_state_mapping: {
    numeric_exact_or_bounded: 'comparable',
    missing_invalid_unavailable_incompatible_or_semantic_mismatch: 'not_comparable',
    zero_baseline: 'absolute_delta_preserved_percentage_undefined',
  },
};
const frozenFindingPolicy = {
  absence_requires: 'one_complete_or_partial_not_triggered_evaluation_same_rule_version_subject_engine_version_pack_and_compatible_registry_no_missing_prerequisites',
  absence_unproven: 'incomparable_in_both_directions',
  partial_evidence: 'resolved_or_added_entries_where_finding_or_unique_negative_evaluation_is_partial',
  algorithm_changed: 'rule_id_evaluator_version_or_stable_subject_mismatch',
  evaluator_version_policy: {
    identity_fields: ['rule_id', 'evaluator_version', 'stable_subject_ref'],
    match: 'exact',
    mismatch: 'algorithm_changed',
  },
  stable_subject_required: true,
};
if (canonical(comparatorManifest.metric_policy) !== canonical(frozenMetricPolicy)
  || canonical(comparatorManifest.finding_policy) !== canonical(frozenFindingPolicy)) {
  throw new Error('comparator typed metric/finding policy drifted');
}
if (canonical(comparatorManifest.reason_policy) !== canonical({ order: 'first_occurrence', duplicates: 'removed' })) {
  throw new Error('comparator reason policy drifted');
}
const parallelismPolicy = comparatorManifest.parallelism_policy;
if (canonical(parallelismPolicy?.gated_metrics) !== canonical([
  { metric_id: 'QueryCumulativeCpuTime', subject_match: 'query' },
  { metric_id: 'QuerySpillBytes', subject_match: 'query' },
]) || parallelismPolicy.different_counts !== 'not_comparable:parallelism_shape_mismatch'
  || parallelismPolicy.unavailable_counts !== 'not_comparable:parallelism_shape_unavailable') {
  throw new Error('comparator parallelism policy drifted');
}
const componentManifest = JSON.parse(await read(fixtureManifest.analysis_component_manifest.path));
if (componentManifest.manifest_hash !== canonicalHash(without(componentManifest, 'manifest_hash'))) {
  throw new Error('analysis component manifest hash mismatch');
}
if (fixtureManifest.analysis_component_manifest.sha256 !== componentManifest.manifest_hash) {
  throw new Error('fixture manifest analysis component link mismatch');
}
if (!['not_ready', 'partial_implementation', 'ready'].includes(componentManifest.implementation_status)) {
  throw new Error('analysis component manifest has an unknown implementation status');
}
const allowedComponentStatuses = new Set(['not_ready', 'implemented_pending_pack_sync', 'implemented_pending_e2e', 'implemented']);
for (const component of componentManifest.components) {
  if (component.manifest_hash !== canonicalHash(without(component, 'manifest_hash'))) {
    throw new Error(`analysis component hash mismatch: ${component.id}`);
  }
  const presentationOnly = component.id === 'sr3514-graph-resolver-contract'
    && component.implementation_status === 'plan_presentation_hotspot_implemented_runtime_not_ready';
  if (!allowedComponentStatuses.has(component.implementation_status) && !presentationOnly) throw new Error(`${component.id} has an unknown implementation status`);
}
const runtimeAdapter = componentManifest.components.find((component) => component.id === 'sr3514-runtime-profile-adapter');
if (!runtimeAdapter || runtimeAdapter.version !== '0.2.3' || runtimeAdapter.implementation_status !== 'implemented_pending_e2e') {
  throw new Error('real RuntimeProfile adapter status/version is missing');
}
const graphResolver = componentManifest.components.find((component) => component.id === 'sr3514-graph-resolver-contract');
if (!graphResolver || graphResolver.version !== '0.10.0'
  || graphResolver.implementation_status !== 'implemented_pending_e2e'
  || canonical(Object.keys(graphResolver.material_hashes ?? {}).sort()) !== canonical(['cross_fragment_membership_fixture', 'graph_cases', 'order_recipe_cases', 'recipe_manifest'])) {
  throw new Error('v0.6 graph resolver receipt must bind only graph/order cases, recipe and cross-fragment fixture');
}
const expectedReceipt = componentManifest.components.map(({ id, version, manifest_hash, material_hashes }) => ({
  id,
  version,
  manifest_hash: id === 'starrocks-3.5.14'
    ? material_hashes.version_pack_semantic
    : id === 'doctor-v0.1' ? material_hashes.rule_set_semantic : manifest_hash,
}));
for (const fixture of fixtureManifest.fixtures) {
  const inputBytes = await readBytes(fixture.input_path);
  const input = inputBytes.toString('utf8');
  const expected = JSON.parse(await read(fixture.expected_path));
  if (sha256(inputBytes) !== fixture.input_sha256) throw new Error(`${fixture.fixture_id} input hash mismatch`);
  if (canonicalHash(expected) !== fixture.expected_sha256) throw new Error(`${fixture.fixture_id} expected hash mismatch`);
  if (expected.input.source_hash !== fixture.input_sha256) throw new Error(`${fixture.fixture_id} report source hash mismatch`);
  if (expected.input.byte_length !== inputBytes.length) throw new Error(`${fixture.fixture_id} report byte length mismatch`);
  const expectedLineCount = fixture.source_kind === 'text_profile' ? input.split(/\r?\n/).filter((_, index, lines) => index < lines.length - 1 || lines[index].length > 0).length : null;
  if (expected.input.line_count !== expectedLineCount) throw new Error(`${fixture.fixture_id} report line count mismatch`);
  if (canonical(expected.analysis_receipt.components) !== canonical(expectedReceipt)) {
    throw new Error(`${fixture.fixture_id} does not carry the six frozen analysis components`);
  }
  if (expected.input.detected_kind !== fixture.source_kind || expected.source_view.source_kind !== fixture.source_kind) {
    throw new Error(`${fixture.fixture_id} source kind does not match its report`);
  }
  for (const [key, value] of Object.entries(fixture.expected_version_context)) {
    if (expected.version[key] !== value) throw new Error(`${fixture.fixture_id} version.${key} mismatch`);
  }
  if (!fixture.expected_facts.length || !fixture.expected_absences.length) throw new Error(`${fixture.fixture_id} lacks fact/absence assertions`);
}
const [jsonFixture, textFixture] = await Promise.all(fixtureManifest.fixtures.map(async (fixture) => JSON.parse(await read(fixture.expected_path))));
if (jsonFixture.analysis_key === textFixture.analysis_key) throw new Error('source-specific analysis keys collapsed');
if (jsonFixture.facts_hash !== textFixture.facts_hash) throw new Error('lossless JSON/Text facts hashes differ');
if (jsonFixture.operator_rows.flatMap((row) => row.metric_cells).length === 0
  || textFixture.operator_rows.flatMap((row) => row.metric_cells).length === 0) {
  throw new Error('JSON/Text equivalence fixtures contain no metric facts');
}
for (const fixture of [jsonFixture, textFixture]) {
  // Retained synthetic examples predate the summary: it must decode as an
  // explicit unavailable state, never as an empty "no hotspot" result.
  if (fixture.execution_summary !== undefined && fixture.execution_summary.state !== 'unavailable') {
    throw new Error('synthetic unavailable graph acquired a fabricated execution summary');
  }
}

const tables = new Map();
for (const table of fixtureManifest.contract_case_tables) {
  const value = JSON.parse(await read(table.path));
  if (canonicalHash(value) !== table.sha256) throw new Error(`${table.case_table_id} hash mismatch`);
  tables.set(table.case_table_id, value);
}
if (fixtureManifest.manifest_hash !== canonicalHash(without(fixtureManifest, 'manifest_hash'))) {
  throw new Error('fixture manifest hash mismatch');
}

const pack = JSON.parse(await read('backend/crates/version-packs/starrocks_3_5_14/version-pack.json'));
const applicableRegistry = `${pack.metric_registry.registry_id}@${pack.metric_registry.version}`;
for (const fixture of fixtureManifest.fixtures) {
  if (fixture.applicable_registry !== applicableRegistry) {
    throw new Error(`fixture registry link mismatch: ${fixture.fixture_id}`);
  }
}
const recipeHash = canonicalHash(without(pack.metric_recipe_manifest, 'manifest_hash'));
if (pack.metric_recipe_manifest.manifest_hash !== recipeHash) throw new Error('metric recipe manifest hash mismatch');
const packSemantic = without(pack, 'manifest_hash', 'semantic_manifest_hash', 'golden_manifest_path', 'golden_manifest_hash', 'golden_manifest_hash_scope');
if (pack.semantic_manifest_hash !== canonicalHash(packSemantic)) throw new Error('version pack semantic hash mismatch');
if (pack.golden_manifest_hash !== fixtureManifest.manifest_hash) throw new Error('version pack golden link mismatch');
if (pack.manifest_hash !== canonicalHash(without(pack, 'manifest_hash'))) throw new Error('version pack manifest hash mismatch');
if (pack.upstream_commit !== '23a56ec517db763ea9bc7ee50b19357acdee0303') throw new Error('version pack upstream commit drifted');
const versionPackSource = await read('backend/crates/version-packs/starrocks_3_5_14/src/lib.rs');
const sourceHashConstant = (name) => new RegExp(
  `pub const ${name}: &str\\s*=\\s*"([0-9a-f]{64})"\\s*;`,
).exec(versionPackSource)?.[1];
if (sourceHashConstant('OUTER_MANIFEST_HASH') !== pack.manifest_hash
  || sourceHashConstant('GOLDEN_MANIFEST_HASH') !== pack.golden_manifest_hash
  || sourceHashConstant('SEMANTIC_MANIFEST_HASH') !== pack.semantic_manifest_hash) {
  throw new Error('version pack source hash constants do not match version-pack.json');
}
for (const evidence of pack.source_evidence) {
  if (!/^[a-f0-9]{64}$/.test(evidence.source_blob_sha256)) throw new Error(`invalid source evidence hash: ${evidence.path}`);
}
const registeredMetrics = new Set(pack.metric_registry.metrics.map((metric) => metric.metric_id));
for (const recipe of pack.metric_recipe_manifest.recipes) {
  for (const input of recipe.typed_inputs) if (!registeredMetrics.has(input)) throw new Error(`${recipe.id} input is not registered: ${input}`);
}

const ruleset = JSON.parse(await read('backend/crates/rule-sets/doctor_v0_1/ruleset.json'));
if (!ruleset.registry_requirements.includes(applicableRegistry)) {
  throw new Error('ruleset registry requirement mismatch');
}
if (ruleset.semantic_manifest_hash !== canonicalHash(without(ruleset, 'manifest_hash', 'semantic_manifest_hash'))) {
  throw new Error('ruleset semantic hash mismatch');
}
if (ruleset.manifest_hash !== canonicalHash(without(ruleset, 'manifest_hash'))) throw new Error('ruleset manifest hash mismatch');
const componentById = (id) => componentManifest.components.find((component) => component.id === id);
if (componentById('sr3514-metric-registry').version !== pack.metric_registry.version) {
  throw new Error('metric registry component version mismatch');
}
if (componentById('starrocks-3.5.14').version !== pack.product_pack_version) {
  throw new Error('version pack component version mismatch');
}
if (componentById('doctor-v0.1').version !== ruleset.version) {
  throw new Error('rule set component version mismatch');
}
const comparatorRegistry = comparatorManifest.registry_equivalence;
const acceptedRegistryPair = {
  baseline: {
    id: 'sr3514-metric-registry',
    version: '0.1.0',
    manifest_hash: 'af257364f97f03bca0f0b9b6e4901dd9df871b133e58e623811f0ba210c104a9',
  },
  candidate: {
    id: 'sr3514-metric-registry',
    version: '0.1.1',
    manifest_hash: '850fa6244f1235dbc370b92f8ecf2f53a28789a9214b89f1e41485c7c4098d62',
  },
};
if (comparatorRegistry.recipe_id !== 'sr3514.registry.same_metric'
  || comparatorRegistry.recipe_version !== '1'
  || comparatorRegistry.symmetric !== true
  || canonical(comparatorRegistry.accepted_receipt_pair) !== canonical(acceptedRegistryPair)) {
  throw new Error('comparator registry equivalence receipt recipe mismatch');
}
const eligibleRegistryMetrics = comparatorRegistry.eligible_metrics;
for (const eligible of eligibleRegistryMetrics) {
  const current = pack.metric_registry.metrics.find((metric) => metric.metric_id === eligible.metric_id);
  if (!current || current.dimension !== eligible.dimension || current.canonical_unit !== eligible.canonical_unit) {
    throw new Error(`comparator registry eligible metric drift: ${eligible.metric_id}`);
  }
}
const eligibleRegistryMetric = new Map(eligibleRegistryMetrics.map((metric) => [metric.metric_id, metric]));
const normalizedReceiptPair = (left, right) => [canonical(left), canonical(right)].sort().join('|');
const acceptedRegistryPairMatches = (baseline, candidate) => normalizedReceiptPair(baseline, candidate)
  === normalizedReceiptPair(acceptedRegistryPair.baseline, acceptedRegistryPair.candidate);
const unregisteredRegistry = {
  id: 'unregistered-metric-registry',
  version: '9.9.9',
  manifest_hash: '1111111111111111111111111111111111111111111111111111111111111111',
};
if (!acceptedRegistryPairMatches(acceptedRegistryPair.candidate, acceptedRegistryPair.baseline)
  || acceptedRegistryPairMatches(unregisteredRegistry, acceptedRegistryPair.candidate)) {
  throw new Error('comparator registry receipt equivalence is not symmetric and closed');
}
const adapterMaterial = componentById('sr3514-runtime-profile-adapter').material_hashes;
const adapterContract = {
  accepted_source_kinds: ['json_profile', 'text_profile'],
  equivalence: 'canonical_facts_exclude_source_view_and_provenance',
  official_formatter_shapes: ['RuntimeProfile.JsonProfileFormatter', 'RuntimeProfile.prettyPrint'],
  runtime_profile_fixture_sha256: adapterMaterial.runtime_profile_text,
  execution_summary: {
    recipe: 'sr3514.explain_analyzer.node_info.v1',
    operator_role: 'absent CommonMetrics.IsSubordinate is native (ExplainAnalyzer.NodeIterator)',
    time: 'sum max(OperatorTotalTime) native+subordinate, plus first native NetworkTime or ScanTime',
    share: 'QueryCumulativeOperatorTime; strict >30% most, >15% second',
    rows: 'version-pack OutputRows recipe, producing (non-sink) fragment group',
  },
  physical_operator_membership: {
    identity: 'typed_pipeline_container_reference_captured_at_parse_time',
    occurrence: 'source_order_per_fragment_pipeline',
    missing: 'explicit_none_no_opaque_id_inference',
    drivers: 'share_enclosing_pipeline_container_not_plan_group_first_pipeline',
  },
  public_evidence_refs: {
    rule_input_projection: 'observation_id_to_metric_cell_ref',
    missing_or_ambiguous: 'projection_error',
    collision_suffix: 'canonical_observation_id_hash',
  },
  fixture_only: false,
};
if (adapterMaterial.adapter_contract !== canonicalHash(adapterContract)) {
  throw new Error('adapter contract material hashes mismatch');
}
if (componentById('strict-metric-parser-contract').material_hashes.metric_cases !== canonicalHash(tables.get('sr3514_metric_contract'))) {
  throw new Error('metric parser component material hash mismatch');
}
if (componentById('sr3514-metric-registry').material_hashes.metric_registry !== canonicalHash(pack.metric_registry)) {
  throw new Error('metric registry component material hash mismatch');
}
const graphMaterial = componentById('sr3514-graph-resolver-contract').material_hashes;
if (graphMaterial.graph_cases !== canonicalHash(tables.get('sr3514_graph_contract'))
  || graphMaterial.order_recipe_cases !== canonicalHash(tables.get('sr3514_order_recipe_contract'))
  || graphMaterial.recipe_manifest !== canonicalHash(without(pack.metric_recipe_manifest, 'manifest_hash'))
  || graphMaterial.cross_fragment_membership_fixture !== sha256(await readBytes('fixtures/synthetic/3.5.14/sr3514_cross_fragment_exchange.profile.txt'))) {
  throw new Error('graph resolver component material hashes mismatch');
}
if (componentById('starrocks-3.5.14').material_hashes.version_pack_semantic !== pack.semantic_manifest_hash) {
  throw new Error('version pack component material hash mismatch');
}
if (componentById('doctor-v0.1').material_hashes.rule_set_semantic !== ruleset.semantic_manifest_hash) {
  throw new Error('rule set component material hash mismatch');
}

const metricCases = tables.get('sr3514_metric_contract');
requireIds(metricCases, [
  'duration_500ms', 'duration_500us', 'duration_500ns', 'duration_1m2ms', 'duration_1m30s',
  'duration_1h30m', 'duration_2s345ms', 'duration_micro_greek_rejected',
  'duration_micro_sign_rejected', 'bytes_1024', 'bytes_half_up_tie', 'bytes_iec_rejected',
  'count_1000', 'count_parenthetical_exact', 'ratio_percent', 'i64_max', 'duration_overflow',
  'intermediate_overflow', 'missing_value', 'compat_partial_no_value', 'exact_zero',
], 'metric');
if (byId(metricCases, 'bytes_1024').canonical_number !== '1024') throw new Error('bytes contract is not 1024 based');
if (byId(metricCases, 'duration_500ms').canonical_number !== '500000000') throw new Error('500ms contract regressed');
if (byId(metricCases, 'compat_partial_no_value').canonical_number !== null) throw new Error('partial metric acquired a fake value');
for (const state of ['missing', 'invalid', 'partial', 'overflow']) {
  if (!metricCases.some((item) => item.parse_state === state)) throw new Error(`metric state missing: ${state}`);
}
if (byId(metricCases, 'exact_zero').canonical_number !== '0') throw new Error('exact zero is not explicit');
const lossyJson = byId(metricCases, 'lossy_pair_json_exact');
const lossyText = byId(metricCases, 'lossy_pair_text_interval');
if (lossyJson.pair_id !== lossyText.pair_id) throw new Error('lossy JSON/Text pair IDs differ');
const lossyExact = BigInt(lossyJson.canonical_number);
const lossyLower = BigInt(lossyText.bounds.lower);
const lossyUpper = BigInt(lossyText.bounds.upper);
if (!(lossyExact >= lossyLower && lossyExact < lossyUpper)) throw new Error('lossy text interval does not contain exact JSON value');

const graphCases = tables.get('sr3514_graph_contract');
requireIds(graphCases, ['graph_explicit', 'graph_unavailable'], 'graph');
if (byId(graphCases, 'graph_unavailable').edges.length !== 0) throw new Error('unavailable graph contains fabricated edges');
if (graphCases.some((entry) => entry.edge_source === 'verified_derived')) throw new Error('no production path derives a graph edge');

const ruleCases = tables.get('sr3514_rule_contract');
const expectedRuleIds = new Set([
  'SR3514.SCAN_TIME_SHARE',
  'SR3514.JOIN_TIME_SHARE',
  'SR3514.AGGREGATE_TIME_SHARE',
  'SR3514.EXCHANGE_NETWORK_TIME_SHARE',
  'SR3514.SPILL_OBSERVED',
  'SR3514.ROW_FLOW_AMPLIFICATION',
  'SR3514.NODE_TIME_SHARE',
  'SR3514.OPERATOR_TIME_SKEW',
  'SR3514.JOIN_PROBE_FANOUT',
  'SR3514.BROADCAST_EXCHANGE_BYTES',
  'SR3514.STREAMING_PREAGG_INEFFECTIVE',
]);
const manifestRuleIds = new Set(ruleset.rules.map((rule) => rule.rule_id));
if (ruleset.rules.length !== expectedRuleIds.size || manifestRuleIds.size !== expectedRuleIds.size
  || [...expectedRuleIds].some((ruleId) => !manifestRuleIds.has(ruleId))) {
  throw new Error('ruleset must contain exactly the eleven active rules');
}
if (ruleCases.length !== 66) throw new Error(`expected 66 rule golden cases, found ${ruleCases.length}`);
if (new Set(ruleCases.map((item) => item.case_id)).size !== ruleCases.length) {
  throw new Error('rule golden case IDs must be globally unique across rules');
}
if (ruleCases.some((item) => !expectedRuleIds.has(item.rule_id))) {
  throw new Error('rule golden case references an unsupported rule');
}
for (const rule of ruleset.rules) {
  const cases = ruleCases.filter((item) => item.rule_id === rule.rule_id);
  const manifestCaseIds = new Set(rule.golden_case_ids);
  const fixtureCaseIds = new Set(cases.map((item) => item.case_id));
  if (manifestCaseIds.size !== rule.golden_case_ids.length) throw new Error(`${rule.rule_id} manifest golden_case_ids contain duplicates`);
  if (manifestCaseIds.size !== fixtureCaseIds.size
    || [...manifestCaseIds].some((caseId) => !fixtureCaseIds.has(caseId))
    || [...fixtureCaseIds].some((caseId) => !manifestCaseIds.has(caseId))) {
    throw new Error(`${rule.rule_id} manifest and fixture golden cases differ`);
  }
  for (const caseClass of ['positive', 'negative', 'boundary', 'missing', 'version', 'graph_degraded']) {
    if (!cases.some((item) => item.case_class === caseClass)) throw new Error(`${rule.rule_id} lacks ${caseClass} case`);
  }
  if (cases.some((item) => !['complete', 'partial', 'unavailable'].includes(item.evidence_completeness))) {
    throw new Error(`${rule.rule_id} has invalid evidence completeness`);
  }
  for (const field of ['formula_source', 'threshold_source', 'severity_policy', 'confidence_policy']) {
    if (!rule[field] || rule[field].length === 0) throw new Error(`${rule.rule_id} lacks ${field}`);
  }
  const recommendation = rule.recommendation_template;
  if (!recommendation || !recommendation.risk || !recommendation.expected_direction
    || !Array.isArray(recommendation.validation_steps) || recommendation.validation_steps.length === 0) {
    throw new Error(`${rule.rule_id} lacks recommendation_template`);
  }
  for (const input of rule.typed_inputs) if (!registeredMetrics.has(input)) throw new Error(`${rule.rule_id} input is not registered: ${input}`);
}
if (byId(ruleCases, 'scan_time_boundary').status !== 'not_triggered') throw new Error('15% threshold is not strict');
if (byId(ruleCases, 'spill_boundary').status !== 'suspected') throw new Error('spill interval zero state regressed');
if (byId(ruleCases, 'row_flow_boundary').status !== 'not_triggered') throw new Error('row-flow 10x threshold is not strict');
if (manifestRuleIds.has('SR3514.PIPELINE_DRIVER_TIME_SKEW')) {
  throw new Error('pipeline driver skew is deferred without same-container typed evidence');
}

const compareCases = tables.get('sr3514_compare_contract');
const expectedCompareIds = [
  'attempts_both_complete', 'attempts_complete_partial', 'attempts_both_partial', 'compare_equivalent',
  'compare_zero_baseline', 'compare_interval_baseline_contains_zero', 'compare_interval_baseline_excludes_zero',
  'compare_missing_not_zero', 'compare_invalid_not_zero', 'compare_registry_semantic_mismatch',
  'compare_registry_explicit_equivalent', 'compare_algorithm_changed', 'compare_baseline_failed', 'compare_candidate_failed',
];
requireIds(compareCases, expectedCompareIds, 'compare');
if (compareCases.length !== expectedCompareIds.length || new Set(compareCases.map((item) => item.case_id)).size !== expectedCompareIds.length
  || canonical(comparatorManifest.golden_case_ids) !== canonical(expectedCompareIds)) {
  throw new Error('comparator golden-case set mismatch');
}
const requireFields = (value, fields, label) => {
  if (!value || typeof value !== 'object' || Array.isArray(value) || fields.some((field) => !Object.hasOwn(value, field))) {
    throw new Error(`${label} is not field-complete`);
  }
};
const attemptKey = (attempt) => {
  requireFields(attempt, ['kind'], 'comparison attempt');
  if (attempt.kind === 'outcome') {
    requireFields(attempt, ['kind', 'analysis_state', 'facts_hash'], 'outcome attempt');
    if (!/^[a-f0-9]{64}$/.test(attempt.facts_hash)) throw new Error('outcome attempt facts_hash is not SHA-256');
    return { outcome_facts_hash: attempt.facts_hash, error_code: null, error_stage: null };
  }
  if (attempt.kind === 'error') {
    requireFields(attempt, ['kind', 'error'], 'error attempt');
    requireFields(attempt.error, ['code', 'stage', 'message_id', 'recoverability', 'affected_side'], 'error attempt');
    return { outcome_facts_hash: null, error_code: attempt.error.code, error_stage: attempt.error.stage };
  }
  throw new Error('comparison attempt kind is invalid');
};
const validateInterval = (interval, label) => {
  requireFields(interval, ['lower', 'upper', 'lower_inclusive', 'upper_inclusive'], label);
  for (const endpoint of ['lower', 'upper']) requireFields(interval[endpoint], ['kind', 'value', 'scale'], `${label} ${endpoint}`);
  if (interval.lower.kind !== interval.upper.kind) throw new Error(`${label} mixes canonical number kinds`);
};
const validateDelta = (delta) => {
  requireFields(delta, [
    'metric_id', 'subject_match', 'state', 'baseline_metric_cell_ref', 'candidate_metric_cell_ref', 'absolute_delta',
    'percentage_state', 'percentage_value', 'fidelity', 'bounds', 'percentage_bounds', 'order_rank', 'limitations',
  ], 'metric delta');
  if (!['comparable', 'not_comparable'].includes(delta.state)) throw new Error('metric delta state is not typed');
  const exact = delta.absolute_delta !== null;
  const interval = delta.bounds !== null;
  if (!((!exact && !interval && delta.fidelity === null)
    || (exact && !interval && delta.fidelity === 'exact')
    || (!exact && interval && delta.fidelity === 'interval'))) {
    throw new Error('metric delta absolute fidelity/bounds matrix mismatch');
  }
  if ((delta.percentage_state === 'computed' && (delta.percentage_value === null || delta.percentage_bounds !== null))
    || (delta.percentage_state === 'approximate' && (delta.percentage_value !== null || delta.percentage_bounds === null))
    || (!['computed', 'approximate'].includes(delta.percentage_state)
      && (delta.percentage_value !== null || delta.percentage_bounds !== null))) {
    throw new Error('metric delta percentage matrix mismatch');
  }
  if (delta.bounds !== null) validateInterval(delta.bounds, 'absolute delta bounds');
  if (delta.percentage_bounds !== null) validateInterval(delta.percentage_bounds, 'percentage delta bounds');
};
for (const compareCase of compareCases) {
  requireFields(compareCase, ['case_id', 'input', 'expected'], 'comparison case');
  requireFields(compareCase.input, ['baseline', 'candidate', 'registry_equivalence_recipe'], 'comparison input');
  for (const side of ['baseline', 'candidate']) {
    const input = compareCase.input[side];
    requireFields(input, ['attempt', 'comparison_projection'], `comparison input ${side}`);
    const key = attemptKey(input.attempt);
    if (input.attempt.kind === 'outcome') {
      const projection = input.comparison_projection;
      requireFields(projection, ['registry', 'structure', 'metric_cells', 'findings'], `comparison projection ${side}`);
      requireFields(projection.registry, ['id', 'version', 'manifest_hash'], `comparison registry ${side}`);
      if (!/^[a-f0-9]{64}$/.test(projection.registry.manifest_hash)) throw new Error(`comparison registry ${side} receipt is invalid`);
      requireFields(projection.structure, ['state', 'stable_entity_ids', 'edge_ids', 'match_recipe'], `comparison structure ${side}`);
      if (!Array.isArray(projection.metric_cells) || !Array.isArray(projection.findings)) throw new Error(`comparison projection ${side} collections are invalid`);
      for (const metric of projection.metric_cells) {
        requireFields(metric, ['metric_cell_ref', 'metric_id', 'subject_ref', 'state', 'canonical_number', 'canonical_unit', 'fidelity', 'bounds', 'order_rank'], `comparison metric ${side}`);
      }
    } else if (input.comparison_projection !== null) {
      throw new Error(`error comparison input ${side} retains a projection`);
    }
    if (canonical(input.attempt) !== canonical(compareCase.expected[side])
      || canonical(key) !== canonical(attemptKey(compareCase.expected[side]))) {
      throw new Error(`${compareCase.case_id} expected ${side} attempt drifted from its input`);
    }
  }
  const baselineProjection = compareCase.input.baseline.comparison_projection;
  const candidateProjection = compareCase.input.candidate.comparison_projection;
  if (baselineProjection !== null && candidateProjection !== null) {
    const recipe = compareCase.input.registry_equivalence_recipe;
    const acceptedPair = acceptedRegistryPairMatches(baselineProjection.registry, candidateProjection.registry);
    if (recipe !== null) {
      requireFields(recipe, ['id', 'version'], `comparison recipe ${compareCase.case_id}`);
      if (recipe.id !== comparatorRegistry.recipe_id || recipe.version !== comparatorRegistry.recipe_version || !acceptedPair) {
        throw new Error(`${compareCase.case_id} declares an unregistered registry equivalence`);
      }
      const metricKey = (metric) => {
        const eligible = eligibleRegistryMetric.get(metric.metric_id);
        if (!eligible || eligible.canonical_unit !== metric.canonical_unit) throw new Error(`${compareCase.case_id} recipe metric is not registry-eligible`);
        return `${eligible.metric_id}|${eligible.dimension}|${eligible.canonical_unit}`;
      };
      const baselineMetrics = baselineProjection.metric_cells.map(metricKey).sort();
      const candidateMetrics = candidateProjection.metric_cells.map(metricKey).sort();
      if (!baselineMetrics.length || canonical(baselineMetrics) !== canonical(candidateMetrics)) {
        throw new Error(`${compareCase.case_id} recipe metric/dimension/unit mismatch`);
      }
    } else if (acceptedPair) {
      throw new Error(`${compareCase.case_id} omits an explicit registry equivalence recipe`);
    }
  }
  const expected = compareCase.expected;
  requireFields(expected, [
    'schema_version', 'comparison_receipt', 'comparison_key', 'baseline', 'candidate', 'state', 'reasons',
    'compatibility_delta', 'structure_delta', 'metric_deltas', 'finding_delta', 'diagnostics', 'limitations',
  ], 'comparison expected');
  if (expected.schema_version !== '2.0'
    || canonical(expected.comparison_receipt) !== canonical({ components: [{
      id: comparatorManifest.id, version: comparatorManifest.version, manifest_hash: comparatorManifest.semantic_manifest_hash,
    }] })) {
    throw new Error(`${compareCase.case_id} comparison receipt or schema drifted`);
  }
  if (expected.comparison_key !== canonicalHash({
    schema_version: expected.schema_version,
    receipt: expected.comparison_receipt,
    baseline: attemptKey(expected.baseline),
    candidate: attemptKey(expected.candidate),
  })) throw new Error(`${compareCase.case_id} comparison key mismatch`);
  requireFields(expected.structure_delta, ['state', 'matches', 'added', 'removed', 'changed', 'edge_delta', 'unresolved'], 'structure delta');
  requireFields(expected.finding_delta, ['added', 'resolved', 'persisting', 'changed', 'incomparable', 'algorithm_changed', 'partial_evidence'], 'finding delta');
  if (![expected.reasons, expected.compatibility_delta, expected.metric_deltas, expected.diagnostics, expected.limitations,
    expected.structure_delta.matches, expected.structure_delta.added, expected.structure_delta.removed,
    expected.structure_delta.changed, expected.structure_delta.edge_delta, expected.structure_delta.unresolved,
    expected.finding_delta.added, expected.finding_delta.resolved, expected.finding_delta.persisting,
    expected.finding_delta.changed, expected.finding_delta.incomparable, expected.finding_delta.algorithm_changed,
    expected.finding_delta.partial_evidence].every(Array.isArray)) {
    throw new Error(`${compareCase.case_id} comparison collection is invalid`);
  }
  for (const delta of expected.metric_deltas) validateDelta(delta);
}
if (canonical(byId(compareCases, 'compare_registry_semantic_mismatch').input.baseline.comparison_projection.registry) !== canonical(unregisteredRegistry)) {
  throw new Error('registry mismatch fixture is not an unregistered receipt');
}
if (!acceptedRegistryPairMatches(
  byId(compareCases, 'compare_registry_explicit_equivalent').input.baseline.comparison_projection.registry,
  byId(compareCases, 'compare_registry_explicit_equivalent').input.candidate.comparison_projection.registry,
)) throw new Error('explicit registry equivalence fixture does not use the accepted receipt pair');
for (const caseId of ['compare_interval_baseline_contains_zero', 'compare_interval_baseline_excludes_zero']) {
  if (byId(compareCases, caseId).expected.metric_deltas[0].state !== 'comparable') {
    throw new Error(`${caseId} represents boundedness through outer comparison/percentage state, not MetricDelta.state`);
  }
}
const exactRatioDelta = byId(compareCases, 'compare_registry_explicit_equivalent').expected.metric_deltas[0];
if (exactRatioDelta.percentage_value?.value !== '1') {
  throw new Error('exact percentage ratio must be (candidate-baseline)/baseline without times 100');
}
const boundedRatio = byId(compareCases, 'compare_interval_baseline_excludes_zero').expected.metric_deltas[0].percentage_bounds;
if (boundedRatio?.lower?.value !== '-0.8' || boundedRatio?.upper?.value !== '9.0') {
  throw new Error('bounded percentage ratio must use conservative fractional-ratio bounds');
}
const zeroClosureRatio = byId(compareCases, 'compare_interval_baseline_contains_zero').expected.metric_deltas[0];
if (zeroClosureRatio.percentage_state !== 'undefined_baseline_interval_contains_zero'
  || zeroClosureRatio.percentage_bounds !== null) {
  throw new Error('baseline closure containing zero must not emit percentage bounds');
}

const versionCases = tables.get('sr3514_version_profile_contract');
requireIds(versionCases, ['version_detected_3514', 'version_user_declared_3514', 'version_unknown', 'profile_level_2', 'profile_running', 'profile_unmerged'], 'version/profile');
if (byId(versionCases, 'version_user_declared_3514').support_status !== 'parse_only') throw new Error('user declaration incorrectly proves support');
if (byId(versionCases, 'version_unknown').pack_match !== 'no_pack') throw new Error('unknown version fell back to a pack');
for (const id of ['profile_level_2', 'profile_running', 'profile_unmerged']) {
  if (byId(versionCases, id).analysis_state !== 'partial') throw new Error(`${id} incorrectly became complete`);
}

const orderCases = tables.get('sr3514_order_recipe_contract');
requireIds(orderCases, ['multi_operator_plan_node', 'ordered_join_children', 'ordered_sink_ids', 'ordered_operator_profiles', 'output_rows_sink', 'output_rows_union', 'output_rows_general', 'output_rows_broadcast_exchange', 'output_rows_unknown'], 'order/recipe');
if (byId(orderCases, 'ordered_join_children').children.map((item) => item.ref).join(',') !== 'child-z,child-a') throw new Error('source order was replaced by ID order');
if (byId(orderCases, 'multi_operator_plan_node').deduplicate_by_plan_node !== false) throw new Error('multi-operator plan node contract regressed');
if (byId(orderCases, 'output_rows_unknown').state !== 'unavailable') throw new Error('unknown output-row recipe acquired a value');

const budgetCases = tables.get('sr3514_budget_contract');
for (const item of budgetCases) if (item.over_limit !== item.limit + 1) throw new Error(`${item.budget_id} is not a boundary+1 case`);

const exportFixtureText = await read('contracts/generated/export-contract-fixtures.ts');
for (const fixture of ['sr3514ExportAnalysis', 'sr3514ExportComparisonFailedCandidate']) {
  if (!exportFixtureText.includes(`export const ${fixture}`)) throw new Error(`export generated fixture missing: ${fixture}`);
}
if ((exportFixtureText.match(/satisfies ExportArtifact/g) ?? []).length !== 2) {
  throw new Error('both generated export fixtures must be checked by TypeScript');
}
const exportManifest = JSON.parse(await read('fixtures/manifests/sr3514-export-contract-fixtures.json'));
const exportArtifactSchema = JSON.parse(await read('contracts/schema/ExportArtifact.schema.json'));
const exportManifestSchema = JSON.parse(await read('contracts/schema/ExportManifest.schema.json'));
if (exportArtifactSchema.properties?.schema_version?.const !== '0.1'
  || exportManifestSchema.properties?.schema_version?.const !== '0.1') {
  throw new Error('export schema_version is not fixed to 0.1');
}
if (exportManifest.schema_version !== '0.1'
  || exportManifest.evidence !== 'synthetic_contract_only_not_docker_e2e'
  || exportManifest.fixture_family !== 'starrocks-3.5.14-synthetic-export-contract'
  || exportManifest.manifest_hash !== canonicalHash(without(exportManifest, 'manifest_hash'))) {
  throw new Error('export fixture manifest is not deterministic synthetic contract evidence');
}
const validSha256 = (value) => typeof value === 'string' && /^[0-9a-f]{64}$/i.test(value);
const exportForbiddenCanaries = [
  'EXPORT_SECRET_CANARY', 'password-canary', 'sql-canary', 'raw-profile-canary', 'absolute-path-canary',
];
const walkStrings = (value, visit) => {
  if (typeof value === 'string') visit(value);
  else if (Array.isArray(value)) value.forEach((item) => walkStrings(item, visit));
  else if (value && typeof value === 'object') Object.values(value).forEach((item) => walkStrings(item, visit));
};
for (const fixture of exportManifest.fixtures) {
  requireFields(fixture, ['fixture_id', 'artifact_kind', 'fixture_path', 'fixture_sha256', 'subject_hash_kind'], 'export fixture manifest item');
  const artifactBytes = await readBytes(fixture.fixture_path);
  const artifact = JSON.parse(artifactBytes.toString('utf8'));
  if (fixture.fixture_sha256 !== sha256(artifactBytes)) throw new Error(`${fixture.fixture_id} export artifact hash mismatch`);
  if (artifact.schema_version !== '0.1'
    || artifact.subject?.kind !== fixture.artifact_kind
    || artifact.manifest?.schema_version !== '0.1'
    || artifact.manifest?.kind !== fixture.artifact_kind
    || artifact.manifest?.evidence !== 'synthetic_contract_only_not_docker_e2e'
    || artifact.manifest?.canonicalization !== 'doctor-canonical-json-v1'
    || artifact.manifest?.integrity !== 'self_consistent_not_signed'
    || artifact.manifest?.subject_hashes?.kind !== fixture.subject_hash_kind) {
    throw new Error(`${fixture.fixture_id} export kind/version/evidence drifted`);
  }
  const manifest = artifact.manifest;
  if (!Array.isArray(manifest.files) || manifest.files.length !== 1
    || manifest.files[0].path !== 'report.json'
    || manifest.files[0].media_type !== 'application/vnd.sr-doctor.report+json;version=0.1'
    || manifest.files[0].byte_length <= 0
    || !validSha256(manifest.files[0].sha256)
    || !validSha256(manifest.manifest_hash)) {
    throw new Error(`${fixture.fixture_id} report manifest is not field-complete`);
  }
  if (!Array.isArray(manifest.schemas) || manifest.schemas.length === 0
    || manifest.schemas.some((schema) => !schema.schema_id || !schema.version || !validSha256(schema.sha256))) {
    throw new Error(`${fixture.fixture_id} schema receipts are invalid`);
  }
  if (!Array.isArray(manifest.receipt_bindings)
    || manifest.receipt_bindings.filter((binding) => binding.scope === 'exporter').length !== 1
    || manifest.receipt_bindings.some((binding) => !['exporter', 'analysis', 'comparison', 'baseline', 'candidate'].includes(binding.scope)
      || !binding.component?.id || !binding.component?.version || !validSha256(binding.component?.manifest_hash))) {
    throw new Error(`${fixture.fixture_id} receipt bindings are invalid`);
  }
  const exporterBindings = manifest.receipt_bindings.filter((binding) => binding.scope === 'exporter');
  if (exporterBindings[0].component.id !== 'doctor-export-v0.1') throw new Error(`${fixture.fixture_id} exporter receipt drifted`);
  if (fixture.artifact_kind === 'analysis') {
    if (manifest.subject_hashes.analysis_key !== artifact.subject.snapshot.analysis_key
      || manifest.subject_hashes.facts_hash !== artifact.subject.snapshot.facts_hash) {
      throw new Error('analysis export subject hashes do not match snapshot');
    }
  } else {
    const snapshot = artifact.subject.snapshot;
    if (manifest.subject_hashes.comparison_key !== snapshot.comparison_key
      || snapshot.candidate?.error === undefined
      || snapshot.candidate?.outcome !== undefined
      || manifest.subject_hashes.candidate?.state !== 'failed'
      || Object.keys(manifest.subject_hashes.candidate).sort().join(',') !== 'error_state,state') {
      throw new Error('comparison export does not retain the failed-attempt boundary');
    }
    const baselineHash = manifest.subject_hashes.baseline;
    if (!['success', 'failed'].includes(baselineHash?.state)) throw new Error('comparison baseline hash arm is invalid');
  }
  if (manifest.manifest_hash !== canonicalHash(without(manifest, 'manifest_hash'))) {
    throw new Error(`${fixture.fixture_id} manifest hash mismatch`);
  }
  for (const forbidden of exportForbiddenCanaries) {
    walkStrings(artifact, (value) => {
      if (value.includes(forbidden)) throw new Error(`${fixture.fixture_id} contains forbidden canary`);
    });
  }
}
if (exportManifest.fixtures.length !== 2
  || new Set(exportManifest.fixtures.map((fixture) => fixture.artifact_kind)).size !== 2) {
  throw new Error('export fixtures must cover analysis and comparison subjects');
}

console.log(`contracts:test passed (${fixtureManifest.fixtures.length} reports, ${metricCases.length} metrics, ${graphCases.length} graphs, ${ruleCases.length} rules, ${compareCases.length} comparisons, ${versionCases.length} version/profile, ${orderCases.length} order/recipe, ${budgetCases.length} budgets, ${exportManifest.fixtures.length} exports)`);
