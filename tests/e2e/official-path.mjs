function record(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function exactKeys(value, keys) {
  return record(value) && Object.keys(value).length === keys.length && keys.every((key) => Object.hasOwn(value, key));
}

function nonEmptyString(value) {
  return typeof value === "string" && value.length > 0;
}

function positiveGeneration(value) {
  return Number.isSafeInteger(value) && value > 0;
}

function nodeIds(value) {
  return Array.isArray(value) && value.every((id) => Number.isSafeInteger(id) && id >= 0 && id <= 2147483647);
}

/** Exact API route only; callers never infer an official endpoint from a suffix. */
export function officialAnalysisConnectionId(rawPath) {
  const match = /^\/api\/v1\/connections\/([^/]+)\/profiles\/official-analysis$/.exec(String(rawPath));
  if (!match) return null;
  try {
    return decodeURIComponent(match[1]);
  } catch {
    return null;
  }
}

/**
 * Validates one tagged contract arm and returns a receipt-safe projection.
 * `opaque` must never return the original sensitive identifier.
 */
export function summarizeOfficialProfileIdentity(value, opaque) {
  if (typeof opaque !== "function") return null;
  if (exactKeys(value, ["kind", "discovery_generation"])
    && value.kind === "discovery" && positiveGeneration(value.discovery_generation)) {
    return Object.freeze({ kind: "discovery", discovery_generation: value.discovery_generation, execution_attempt_id_sha256: null });
  }
  if (exactKeys(value, ["kind", "execution_attempt_id"])
    && value.kind === "verified_standard_attempt" && nonEmptyString(value.execution_attempt_id)) {
    const executionAttemptIdHash = opaque(value.execution_attempt_id);
    if (!nonEmptyString(executionAttemptIdHash) || executionAttemptIdHash === value.execution_attempt_id) return null;
    return Object.freeze({ kind: "verified_standard_attempt", discovery_generation: null, execution_attempt_id_sha256: executionAttemptIdHash });
  }
  return null;
}

/** Rejects the retired top-level generation form rather than accepting it as discovery. */
export function summarizeOfficialProfileRequest(value, opaque) {
  if (!exactKeys(value, ["connection_id", "query_id", "identity", "node_ids"])
    || !nonEmptyString(value.connection_id) || !nonEmptyString(value.query_id) || !nodeIds(value.node_ids)) return null;
  const identity = summarizeOfficialProfileIdentity(value.identity, opaque);
  if (!identity) return null;
  const connectionIdHash = opaque(value.connection_id);
  const queryIdHash = opaque(value.query_id);
  if (!nonEmptyString(connectionIdHash) || connectionIdHash === value.connection_id
    || !nonEmptyString(queryIdHash) || queryIdHash === value.query_id) return null;
  return Object.freeze({
    connection_id_sha256: connectionIdHash,
    query_id_sha256: queryIdHash,
    identity,
    node_ids: Object.freeze([...value.node_ids]),
  });
}

/** Response query generation is real for discovery and explicitly null for a standard attempt. */
export function summarizeOfficialProfileResponse(value, opaque) {
  if (!record(value) || !record(value.query) || !nodeIds(value.node_ids)
    || !nonEmptyString(value.query.connection_id) || !nonEmptyString(value.query.query_id)) return null;
  const identity = summarizeOfficialProfileIdentity(value.identity, opaque);
  if (!identity) return null;
  const queryGeneration = value.query.discovery_generation;
  const generationMatches = identity.kind === "discovery"
    ? queryGeneration === identity.discovery_generation
    : queryGeneration === null;
  if (!generationMatches) return null;
  const connectionIdHash = opaque(value.query.connection_id);
  const queryIdHash = opaque(value.query.query_id);
  if (!nonEmptyString(connectionIdHash) || connectionIdHash === value.query.connection_id
    || !nonEmptyString(queryIdHash) || queryIdHash === value.query.query_id) return null;
  return Object.freeze({
    kind: value.kind,
    connection_id_sha256: connectionIdHash,
    query_id_sha256: queryIdHash,
    discovery_generation: queryGeneration,
    identity,
    node_ids: Object.freeze([...value.node_ids]),
  });
}

/** Builds the only two eligible identities from a fetched Profile response. */
export function summarizeProfileFetchOfficialIdentity(value, opaque) {
  if (!record(value) || !record(value.query) || !nonEmptyString(value.query.connection_id)
    || !nonEmptyString(value.query.query_id)) return null;
  const eligibilityPresent = Object.hasOwn(value, "official_profile_eligibility");
  let identity;
  if (!eligibilityPresent) {
    identity = summarizeOfficialProfileIdentity({ kind: "discovery", discovery_generation: value.query.discovery_generation }, opaque);
  } else {
    const eligibility = value.official_profile_eligibility;
    if (!exactKeys(eligibility, ["execution_attempt_id", "method"])
      || eligibility.method !== "standard_profile" || value.query.discovery_generation !== null) return null;
    identity = summarizeOfficialProfileIdentity({ kind: "verified_standard_attempt", execution_attempt_id: eligibility.execution_attempt_id }, opaque);
  }
  if (!identity) return null;
  const connectionIdHash = opaque(value.query.connection_id);
  const queryIdHash = opaque(value.query.query_id);
  if (!nonEmptyString(connectionIdHash) || connectionIdHash === value.query.connection_id
    || !nonEmptyString(queryIdHash) || queryIdHash === value.query.query_id) return null;
  return Object.freeze({
    connection_id_sha256: connectionIdHash,
    query_id_sha256: queryIdHash,
    discovery_generation: value.query.discovery_generation,
    identity,
  });
}

export function sameOfficialProfileIdentitySummary(left, right) {
  if (!record(left) || !record(right) || left.kind !== right.kind) return false;
  if (left.kind === "discovery") {
    return positiveGeneration(left.discovery_generation) && left.discovery_generation === right.discovery_generation
      && left.execution_attempt_id_sha256 === null && right.execution_attempt_id_sha256 === null;
  }
  if (left.kind === "verified_standard_attempt") {
    return left.discovery_generation === null && right.discovery_generation === null
      && nonEmptyString(left.execution_attempt_id_sha256) && left.execution_attempt_id_sha256 === right.execution_attempt_id_sha256;
  }
  return false;
}

export function officialProfileIdentityEchoes(request, response) {
  return record(request) && record(response)
    && request.connection_id_sha256 === response.connection_id_sha256
    && request.query_id_sha256 === response.query_id_sha256
    && JSON.stringify(request.node_ids) === JSON.stringify(response.node_ids)
    && sameOfficialProfileIdentitySummary(request.identity, response.identity)
    && (request.identity.kind === "discovery"
      ? response.discovery_generation === request.identity.discovery_generation
      : response.discovery_generation === null);
}
