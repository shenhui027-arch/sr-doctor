//! Session-owned family bindings and two-phase publication. No SQL/Profile text
//! enters the evidence cache or the prepared token.
use super::*;
use doctor_contracts::{
    AuditFamilyContext, AuditFamilyDetailReadRequest, AuditFamilyMembership, AuditProfileEvidence,
    AuditProfileEvidenceCoverage, AuditProfileListCheck, audit_family_membership,
};
use std::collections::VecDeque;

#[derive(Default)]
pub(super) struct SuccessfulProfileEvidence {
    ids: VecDeque<String>,
    evicted: bool,
}
impl SuccessfulProfileEvidence {
    pub(super) fn record(&mut self, id: String) {
        if let Some(index) = self.ids.iter().position(|prior| prior == &id) {
            self.ids.remove(index);
        }
        self.ids.push_back(id);
        if self.ids.len() > AUDIT_FAMILY_PROFILE_CANDIDATE_LIMIT {
            self.ids.pop_front();
            self.evicted = true;
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditFamilyDetailWorkload {
    /// Predicted generation, not published until commit consumes the token.
    pub discovery_generation: u32,
    pub scope: AuditScope,
    pub family: AuditFamilyMembership,
    pub session_identity: AuditSessionIdentity,
    pub source: AuditFamilyDetailSource,
    pub profile_evidence_coverage: AuditProfileEvidenceCoverage,
    pub selected_profile_evidence: Option<AuditProfileEvidence>,
    /// Stable unique-ID registration order; all roles sharing an ID use this index.
    pub representative_indices: BTreeMap<String, usize>,
}

/// Private binding fields prevent callers from manufacturing a commit token.
pub struct PreparedAuditFamilyDetail {
    connection_id: String,
    request_scope: AuditScope,
    base_generation: u32,
    family_key: String,
    parent: AuditScope,
    target: AuditFamilyTarget,
    unique_rows: Vec<AuditOccurrenceSourceRow>,
    workload: AuditFamilyDetailWorkload,
}
impl PreparedAuditFamilyDetail {
    pub fn workload(&self) -> &AuditFamilyDetailWorkload {
        &self.workload
    }
}

fn membership(session: &Session, target: &AuditFamilyTarget) -> AuditFamilyMembership {
    audit_family_membership(
        Some(&target.native_digest),
        AuditFamilyContext {
            cluster: Some(format!("session:{}", session.view.connection_id)),
            engine_version: Some(session.view.detected_version.clone()),
            catalog: Some(target.catalog.clone()),
            database: Some(target.database.clone()),
        },
    )
}

pub(super) fn family_bindings<'a>(
    session: &Session,
    rows: impl Iterator<Item = (Option<&'a str>, Option<&'a str>, Option<&'a str>)>,
) -> Result<BTreeMap<String, AuditFamilyTarget>, ConnectorError> {
    let mut bindings = BTreeMap::new();
    for (digest, catalog, database) in rows {
        let (Some(digest), Some(catalog), Some(database)) = (digest, catalog, database) else {
            continue;
        };
        let target = AuditFamilyTarget {
            native_digest: digest.into(),
            catalog: catalog.into(),
            database: database.into(),
        };
        if let AuditFamilyMembership::ExactNativeDigest { family_key, .. } =
            membership(session, &target)
            && bindings
                .insert(family_key, target.clone())
                .is_some_and(|previous| previous != target)
        {
            return Err(audit_response_error("audit_family_key_collision"));
        }
    }
    Ok(bindings)
}
pub(super) fn occurrence_family_bindings(
    session: &Session,
    rows: &[AuditOccurrenceSourceRow],
) -> Result<BTreeMap<String, AuditFamilyTarget>, ConnectorError> {
    family_bindings(
        session,
        rows.iter().map(|row| {
            (
                row.native_digest.as_deref(),
                row.catalog.as_deref(),
                row.database.as_deref(),
            )
        }),
    )
}
pub(super) fn merge_family_bindings(
    target: &mut BTreeMap<String, AuditFamilyTarget>,
    source: BTreeMap<String, AuditFamilyTarget>,
) -> Result<(), ConnectorError> {
    for (key, value) in source {
        if target
            .insert(key, value.clone())
            .is_some_and(|previous| previous != value)
        {
            return Err(audit_response_error("audit_family_key_collision"));
        }
    }
    Ok(())
}
pub(super) fn publish_family_bindings(
    session: &mut Session,
    bindings: BTreeMap<String, AuditFamilyTarget>,
) {
    if let Some(discovery) = session.audit_discovery.as_mut() {
        discovery.families_by_key = bindings;
    }
}

fn invalid_binding(limitation: &'static str) -> ConnectorError {
    ConnectorError::with_limitation(
        ConnectorErrorCode::QueryDiscoveryStale,
        "audit_family_detail",
        limitation,
    )
}
fn bound_target(
    session: &Session,
    generation: u32,
    scope: &AuditScope,
    key: &str,
) -> Result<(AuditScope, AuditFamilyTarget), ConnectorError> {
    let discovery = session
        .audit_discovery
        .as_ref()
        .ok_or_else(|| invalid_binding("audit_family_not_discovered"))?;
    if session.discovery_generation != generation || discovery.generation != generation {
        return Err(invalid_binding("audit_discovery_stale"));
    }
    if &discovery.scope != scope {
        return Err(invalid_binding("audit_scope_mismatch"));
    }
    let target = discovery
        .families_by_key
        .get(key)
        .cloned()
        .ok_or_else(|| invalid_binding("audit_family_not_discovered"))?;
    let parent = match &discovery.selector {
        AuditDiscoverySelector::FamilyDetail { parent, family_key } if family_key == key => {
            parent.clone()
        }
        AuditDiscoverySelector::FamilyDetail { .. } => {
            return Err(invalid_binding("audit_family_not_discovered"));
        }
        _ => scope.clone(),
    };
    Ok((parent, target))
}

type ProfileCandidates = (
    Vec<String>,
    BTreeMap<String, AuditProfileEvidence>,
    AuditProfileEvidenceCoverage,
);

fn profile_candidates(session: &mut Session) -> Result<ProfileCandidates, ConnectorError> {
    let limit = AUDIT_FAMILY_PROFILE_CANDIDATE_LIMIT as u16;
    let mut coverage = AuditProfileEvidenceCoverage {
        profile_list_check: match session.view.capabilities.profile_list {
            CapabilityState::Available => AuditProfileListCheck::Checked,
            CapabilityState::Unavailable => AuditProfileListCheck::CapabilityUnavailable,
            CapabilityState::Forbidden => AuditProfileListCheck::CapabilityForbidden,
            CapabilityState::Unknown => AuditProfileListCheck::CapabilityUnknown,
        },
        profile_list_limit: limit,
        profile_list_returned: 0,
        profile_list_at_limit: false,
        profile_list_excluded: 0,
        successful_cache_retained: session.successful_profiles.ids.len() as u16,
        successful_cache_capacity: limit,
        successful_cache_evicted: session.successful_profiles.evicted,
        candidate_count: 0,
        candidate_limit: limit,
        candidate_truncated: false,
    };
    let mut ids = Vec::new();
    let mut evidence = BTreeMap::new();
    for id in session.successful_profiles.ids.iter().rev() {
        ids.push(id.clone());
        evidence.insert(id.clone(), AuditProfileEvidence::SuccessfulFetchInSession);
    }
    if session.view.capabilities.profile_list == CapabilityState::Available {
        let response = session
            .remote
            .run(RemoteOperation::ShowProfileList { limit })
            .map_err(|error| error.connector("audit_family_detail_profile_list"))?;
        let RemoteResponse::Rows(rows) = response else {
            return Err(audit_response_error("audit_family_profile_list_shape"));
        };
        if rows.len() > usize::from(limit) {
            return Err(audit_response_error("audit_family_profile_list_budget"));
        }
        coverage.profile_list_returned = rows.len() as u16;
        coverage.profile_list_at_limit = rows.len() == usize::from(limit);
        for row in rows {
            // Fixed 3.5.14 SHOW PROFILELIST shape. Drop SQL (column 4) here;
            // only canonical UUID and the Finished state can become evidence.
            if row.len() != 5 {
                return Err(audit_response_error("audit_family_profile_list_shape"));
            }
            let Some(id) =
                canonical_query_id(&row[0]).filter(|_| row[3].eq_ignore_ascii_case("Finished"))
            else {
                coverage.profile_list_excluded += 1;
                continue;
            };
            if evidence.contains_key(&id) {
                continue;
            }
            if ids.len() == AUDIT_FAMILY_PROFILE_CANDIDATE_LIMIT {
                coverage.candidate_truncated = true;
                continue;
            }
            ids.push(id.clone());
            evidence.insert(id, AuditProfileEvidence::ProfileList);
        }
    }
    coverage.candidate_count = ids.len() as u16;
    Ok((ids, evidence, coverage))
}

impl SessionManager {
    pub fn prepare_audit_family_detail(
        &self,
        connection_id: &str,
        request: AuditFamilyDetailReadRequest,
    ) -> Result<PreparedAuditFamilyDetail, ConnectorError> {
        self.prepare_audit_family_detail_controlled(connection_id, request, None)
    }

    pub fn prepare_audit_family_detail_controlled(
        &self,
        connection_id: &str,
        request: AuditFamilyDetailReadRequest,
        control: Option<&Arc<OperationControl>>,
    ) -> Result<PreparedAuditFamilyDetail, ConnectorError> {
        request.scope.validate().map_err(audit_request_error)?;
        let valid_key = request
            .family_key
            .strip_prefix("audit-family-v1:")
            .is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            });
        if request.connection_id != connection_id || !valid_key {
            return Err(ConnectorError::new(
                ConnectorErrorCode::InvalidRequest,
                "audit_family_detail",
            ));
        }
        self.with_supported_session_controlled(connection_id, control, |session| {
            let (parent, target) = bound_target(
                session,
                request.discovery_generation,
                &request.scope,
                &request.family_key,
            )?;
            let next_generation = request
                .discovery_generation
                .checked_add(1)
                .ok_or_else(|| invalid_binding("audit_generation_exhausted"))?;
            let schema = verified_audit_schema(session)?;
            audit_family_detail_statement(&session.audit_table, &parent, &target, &[], &schema)
                .map_err(audit_schema_error)?;
            let (candidates, evidence, coverage) = profile_candidates(session)?;
            // Revalidate with the sealed evidence list before the transport's
            // generic 100KiB protocol guard can turn a local budget into fatal.
            audit_family_detail_statement(
                &session.audit_table,
                &parent,
                &target,
                &candidates,
                &schema,
            )
            .map_err(audit_schema_error)?;
            let source = decode_family_detail_source_rows(
                nullable_rows(
                    session
                        .remote
                        .run(RemoteOperation::AuditFamilyDetail {
                            table: session.audit_table.clone(),
                            parent: parent.clone(),
                            target: target.clone(),
                            candidates: candidates.clone(),
                            schema: schema.clone(),
                        })
                        .map_err(|error| error.connector("audit_family_detail"))?,
                    "audit_family_detail",
                )?,
                &parent,
                &target,
                &candidates,
                &schema,
            )
            .map_err(audit_response_error)?;
            let selected_profile_evidence = source
                .profile_known
                .as_ref()
                .and_then(|row| row.query_id.as_deref())
                .and_then(canonical_query_id)
                .and_then(|id| evidence.get(&id).cloned());
            let mut unique = BTreeMap::new();
            for row in [&source.recent, &source.slowest, &source.profile_known]
                .into_iter()
                .flatten()
            {
                let id = row
                    .query_id
                    .as_deref()
                    .and_then(canonical_query_id)
                    .ok_or_else(|| audit_response_error("audit_family_representative_id"))?;
                if unique
                    .insert(id, row.clone())
                    .is_some_and(|previous| previous != *row)
                {
                    return Err(audit_response_error("audit_family_representative_conflict"));
                }
            }
            let representative_indices = unique
                .keys()
                .enumerate()
                .map(|(index, id)| (id.clone(), index))
                .collect();
            let unique_rows = unique.into_values().collect();
            let workload = AuditFamilyDetailWorkload {
                discovery_generation: next_generation,
                scope: target.family_scope(&parent).map_err(audit_schema_error)?,
                family: membership(session, &target),
                session_identity: audit_session_identity(session),
                source,
                profile_evidence_coverage: coverage,
                selected_profile_evidence,
                representative_indices,
            };
            Ok(PreparedAuditFamilyDetail {
                connection_id: connection_id.into(),
                request_scope: request.scope,
                base_generation: request.discovery_generation,
                family_key: request.family_key,
                parent,
                target,
                unique_rows,
                workload,
            })
        })
    }

    /// Consumes the private prepare token. No remote I/O, parsing or fallible
    /// projection is allowed after the CAS succeeds.
    pub fn commit_audit_family_detail(
        &self,
        prepared: PreparedAuditFamilyDetail,
    ) -> Result<u32, ConnectorError> {
        self.commit_audit_family_detail_controlled(prepared, None)
    }

    pub fn commit_audit_family_detail_controlled(
        &self,
        prepared: PreparedAuditFamilyDetail,
        control: Option<&Arc<OperationControl>>,
    ) -> Result<u32, ConnectorError> {
        self.with_supported_session_controlled(&prepared.connection_id, control, |session| {
            let publish = || {
                let (parent, target) = bound_target(
                    session,
                    prepared.base_generation,
                    &prepared.request_scope,
                    &prepared.family_key,
                )?;
                if parent != prepared.parent || target != prepared.target {
                    return Err(invalid_binding("audit_family_binding_changed"));
                }
                let generation = register_audit_occurrence_discovery_with_selector(
                    session,
                    prepared.workload.scope,
                    &prepared.unique_rows,
                    AuditDiscoverySelector::FamilyDetail {
                        parent,
                        family_key: prepared.family_key.clone(),
                    },
                );
                publish_family_bindings(session, BTreeMap::from([(prepared.family_key, target)]));
                Ok(generation)
            };
            match control {
                Some(control) => control.publish(publish),
                None => publish(),
            }
        })
    }
}
