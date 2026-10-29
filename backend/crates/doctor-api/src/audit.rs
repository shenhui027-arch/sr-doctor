//! Fixed-window audit projections. Remote SQL is sealed in the connector;
//! numeric facts use the core parser, not an HTTP-layer metric parser.

use std::collections::BTreeSet;

use doctor_connector::{
    AUDIT_CONCURRENCY_MAX_STEPS, AUDIT_HISTORICAL_LOOKBACK_SECONDS,
    AUDIT_OVERVIEW_FAMILY_TOP_LIMIT, AuditActiveAtWorkload, AuditConcurrencySource,
    AuditErrorCodeSource, AuditFamilyDetailWorkload, AuditFamilySourceRow, AuditFamilyWorkload,
    AuditFingerprintWorkload, AuditMetricAggregation, AuditOccurrenceSourceRow,
    AuditOccurrenceWorkload, AuditOverviewPrimaryWorkload, AuditOverviewWorkload,
    AuditSessionIdentity, AuditSimilarWorkload, AuditSourceStatementSample, AuditSqlLookupRequest,
    AuditStatementSampleSource, AuditWindowCoverage, audit_historical_source_window,
};
use doctor_contracts::{
    AUDIT_FINGERPRINT_READ_LIMIT, AuditActiveAtParticipant, AuditActiveAtParticipants,
    AuditActiveAtReadRequest, AuditActiveAtResponse, AuditAggregateFact, AuditAggregation,
    AuditBoundedSourceCoverage, AuditBoundedSourcePartialReason, AuditClusterIdentity,
    AuditConcurrencyAxis, AuditConcurrencyCountTick, AuditConcurrencyCoverage,
    AuditConcurrencyPoint, AuditConcurrencyPresentation, AuditConcurrencySeries,
    AuditConcurrencyTimeTick, AuditConcurrencyUnavailableReason, AuditConcurrencyWindow,
    AuditCoverage, AuditErrorCodeFact, AuditErrorProportion, AuditErrorProportionInvalidReason,
    AuditErrorProportionUnavailableReason, AuditFamilyContext, AuditFamilyDetailReadRequest,
    AuditFamilyDetailResponse, AuditFamilyExecutionDistribution, AuditFamilyMembership,
    AuditFamilyPresentation, AuditFamilyReadRequest, AuditFamilyRepresentative,
    AuditFamilyRepresentativeUnavailableReason, AuditFamilyRepresentatives, AuditFamilyResponse,
    AuditFamilySort, AuditFingerprintCoverage, AuditFingerprintFamilyPresentation,
    AuditFingerprintFamilyResponse, AuditFingerprintPartialReason, AuditFingerprintSourceCoverage,
    AuditFingerprintUnavailableCount, AuditMetricCoverage, AuditMetricId, AuditMetricInput,
    AuditMetricState, AuditOccurrencePresentation, AuditOccurrenceReadRequest,
    AuditOccurrenceResponse, AuditOverviewFamilyTopPresentation, AuditOverviewFamilyTopRow,
    AuditOverviewPrimaryPresentation, AuditOverviewProjectionScope, AuditOverviewReadRequest,
    AuditOverviewResponse, AuditPlotNotApplicableReason, AuditPlotProjection,
    AuditPlotUnavailableReason, AuditProfileEvidence, AuditScope, AuditSqlReadRequest,
    AuditStatementSample, AuditStatementSampleUnavailableReason, AuditTopWindow,
    AuditWorkloadValidationError, AuditWorkloadVersion, PRODUCT_SQL_FINGERPRINT_VERSION,
    ProductFingerprintAuthority, audit_family_membership, audit_fingerprint_family_key,
};
use doctor_contracts::{
    AuditFamilyRequest, AuditSimilarSqlReadRequest, AuditSimilarSqlRequest,
    AuditSimilarSqlResponse, SIMILAR_SQL_STRUCTURE_VERSION, SimilarSqlCluster, SimilarSqlCoverage,
    SimilarSqlFamilySource, SimilarSqlMember, SimilarSqlMemberSource,
};
use doctor_core::{
    AuditConcurrencyDelta, AuditFingerprintEvent, AuditFingerprintGroup, AuditFingerprintStatement,
    FamilyCells, MetricCells, SimilarSqlInput, SimilarSqlMemberOut, audit_concurrency_from_deltas,
    cluster_similar_sql, group_audit_fingerprints, normalize_audit_metric,
    sort_audit_fingerprint_groups,
};

use super::*;

pub(crate) mod performance;
pub(super) use performance::performance;

pub(super) async fn occurrences(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: AuditOccurrenceReadRequest = connector_json_request(&state, request).await?;
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    validate_audit_scope_window(&state, &request.request.scope)?;
    let lease = begin_read(&state)?;
    audit_connector_call(
        &state,
        lease,
        connection_id.clone(),
        move |sessions, control, state| {
            let limit = request.request.limit;
            let workload = sessions
                .audit_occurrences_controlled(&connection_id, request.request, Some(control))
                .map_err(|error| connector_failure(state, error))?;
            Ok(no_store(occurrence_response(state, workload, limit)?))
        },
    )
    .await
}

pub(super) async fn families(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: AuditFamilyReadRequest = connector_json_request(&state, request).await?;
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    validate_audit_scope_window(&state, &request.request.scope)?;
    let lease = begin_read(&state)?;
    audit_connector_call(
        &state,
        lease,
        connection_id.clone(),
        move |sessions, control, state| {
            let limit = request.request.limit;
            let workload = sessions
                .audit_families_controlled(&connection_id, request.request, Some(control))
                .map_err(|error| connector_failure(state, error))?;
            Ok(no_store(family_response(state, workload, limit)?))
        },
    )
    .await
}

/// Product-fingerprint families of the digest-missing executions in scope.
/// A separate list: never merged or ranked with exact native-digest families.
pub(super) async fn fingerprint_families(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: AuditFamilyReadRequest = connector_json_request(&state, request).await?;
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    validate_audit_scope_window(&state, &request.request.scope)?;
    let lease = begin_read(&state)?;
    audit_connector_call(
        &state,
        lease,
        connection_id.clone(),
        move |sessions, control, state| {
            let (sort, limit) = (request.request.sort.clone(), request.request.limit);
            let workload = sessions
                .audit_fingerprint_events_controlled(&connection_id, request.request, Some(control))
                .map_err(|error| connector_failure(state, error))?;
            Ok(no_store(fingerprint_family_response(
                state, workload, &sort, limit,
            )?))
        },
    )
    .await
}

pub(super) fn fingerprint_family_response(
    state: &ApiState,
    workload: AuditFingerprintWorkload,
    sort: &AuditFamilySort,
    limit: u16,
) -> Result<AuditFingerprintFamilyResponse, ApiFailure> {
    let identity = workload.session_identity.clone();
    let scope = workload.scope.clone();
    let (mut groups, coverage) = grouped_fingerprints(state, workload)?;
    sort_audit_fingerprint_groups(&mut groups, sort);
    groups.truncate(usize::from(limit));
    let top = top_window(state, limit, groups.len(), &coverage.family_count)?;
    let rows = groups
        .into_iter()
        .map(|group| {
            let context = session_context(&identity, &group.catalog, &group.database);
            let (execution_count, error_count, error_proportion, aggregates) = family_facts(
                state,
                cells_row(&group.catalog, &group.database, &group.cells()),
            )?;
            Ok(AuditFingerprintFamilyPresentation {
                family_key: audit_fingerprint_family_key(&group.fingerprint, &context),
                fingerprint: group.fingerprint,
                template: group.template,
                context,
                execution_count: execution_count.ok_or_else(|| invalid_response(state))?,
                error_count: error_count.ok_or_else(|| invalid_response(state))?,
                error_proportion,
                aggregates,
            })
        })
        .collect::<Result<Vec<_>, ApiFailure>>()?;
    Ok(AuditFingerprintFamilyResponse {
        schema_version: AuditWorkloadVersion::V1,
        connection_id: identity.connection_id.clone(),
        scope,
        cluster_identity: public_identity(&identity),
        audit_timestamp_time_zone: public_time_zone(&identity),
        authority: ProductFingerprintAuthority::ProductUnofficial,
        fingerprint_version: PRODUCT_SQL_FINGERPRINT_VERSION.into(),
        rows,
        coverage,
        top,
        limitations: [
            "product_fingerprint_is_not_an_official_starrocks_digest",
            "product_fingerprint_families_are_separate_from_native_digest_families",
            "fingerprint_read_is_newest_first_and_bounded",
            "truncated_or_unreliable_statements_are_not_grouped_or_ranked",
            "aggregates_cover_read_executions_only",
            "cluster_identity_is_session_scoped_not_cross_connection",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    })
}

/// A2 similar SQL: exact families of one page plus product-fingerprint
/// groups, clustered by an explained structural basis. Read-only; nothing is
/// published to discovery.
pub(super) async fn similar_sql(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: AuditSimilarSqlReadRequest = connector_json_request(&state, request).await?;
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    validate_audit_scope_window(&state, &request.request.scope)?;
    if request.request.validate().is_err() {
        return Err(ApiFailure::new(
            state.next_request_id(),
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "audit_similar",
            "audit_similar_request_invalid",
            "change_input",
        ));
    }
    let lease = begin_read(&state)?;
    audit_connector_call(
        &state,
        lease,
        connection_id.clone(),
        move |sessions, control, state| {
            let similar = request.request;
            let workload = sessions
                .audit_similar_sources_controlled(
                    &connection_id,
                    AuditFamilyRequest {
                        scope: similar.scope.clone(),
                        sort: similar.sort.clone(),
                        limit: similar.limit,
                    },
                    Some(control),
                )
                .map_err(|error| connector_failure(state, error))?;
            Ok(no_store(similar_sql_response(state, workload, &similar)?))
        },
    )
    .await
}

fn decimal<T: std::str::FromStr>(value: &Option<String>) -> Option<T> {
    value
        .as_deref()
        .filter(|text| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|text| text.parse().ok())
}

/// The cells of one SQL family row, as exact integers.
fn family_row_cells(
    state: &ApiState,
    row: &AuditFamilySourceRow,
) -> Result<FamilyCells, ApiFailure> {
    let metric = |valid: &Option<String>, sum: &Option<String>, max: &Option<String>| MetricCells {
        valid_count: decimal(valid).unwrap_or(0),
        sum: decimal(sum),
        max: decimal(max),
    };
    Ok(FamilyCells {
        execution_count: decimal(&row.canonical_execution_count)
            .ok_or_else(|| invalid_response(state))?,
        error_count: decimal(&row.error_count).ok_or_else(|| invalid_response(state))?,
        query_time: metric(
            &row.query_time_valid_count,
            &row.cumulative_query_time,
            &row.max_query_time,
        ),
        scan_bytes: metric(
            &row.scan_bytes_valid_count,
            &row.cumulative_scan_bytes,
            &None,
        ),
        cpu_cost: metric(&row.cpu_cost_valid_count, &row.cumulative_cpu_cost, &None),
        memory_cost: metric(&row.memory_cost_valid_count, &None, &row.max_memory_cost),
    })
}

pub(super) fn similar_sql_response(
    state: &ApiState,
    workload: AuditSimilarWorkload,
    request: &AuditSimilarSqlRequest,
) -> Result<AuditSimilarSqlResponse, ApiFailure> {
    let identity = workload.fingerprint.session_identity.clone();
    let scope = workload.fingerprint.scope.clone();
    let coverage = public_coverage(workload.coverage);
    let top = top_window(
        state,
        request.limit,
        workload.families.len(),
        &coverage.retained_family_count,
    )?;
    let mut inputs = Vec::new();
    for row in &workload.families {
        let AuditFamilyMembership::ExactNativeDigest {
            family_key,
            native_digest,
            ..
        } = membership(
            &identity,
            row.catalog.clone(),
            row.database.clone(),
            row.native_digest.as_deref(),
        )
        else {
            return Err(invalid_response(state));
        };
        let (statement, statement_truncated) = match &row.statement_sample {
            AuditSourceStatementSample::Read(AuditStatementSampleSource::Text {
                value,
                truncated,
            }) => (Some(value.clone()), *truncated),
            _ => (None, false),
        };
        inputs.push(SimilarSqlInput {
            member_key: family_key,
            source: SimilarSqlMemberSource::OfficialDigest { native_digest },
            catalog: row.catalog.clone().unwrap_or_default(),
            database: row.database.clone().unwrap_or_default(),
            statement,
            statement_truncated,
            cells: family_row_cells(state, row)?,
        });
    }
    let (groups, fingerprint) = grouped_fingerprints(state, workload.fingerprint)?;
    for group in groups {
        let context = session_context(&identity, &group.catalog, &group.database);
        inputs.push(SimilarSqlInput {
            member_key: audit_fingerprint_family_key(&group.fingerprint, &context),
            source: SimilarSqlMemberSource::ProductFingerprint {
                fingerprint: group.fingerprint.clone(),
            },
            cells: group.cells(),
            catalog: group.catalog,
            database: group.database,
            statement: Some(group.template),
            statement_truncated: false,
        });
    }
    let member_count = inputs.len();
    let detached: BTreeSet<String> = request.detached_member_keys.iter().cloned().collect();
    let refined: BTreeSet<String> = request.refined_cluster_keys.iter().cloned().collect();
    let out = cluster_similar_sql(
        inputs,
        &request.dimensions,
        &detached,
        &refined,
        &request.sort,
    );
    let member = |member: SimilarSqlMemberOut| -> Result<SimilarSqlMember, ApiFailure> {
        let input = member.input;
        let (execution_count, error_count, error_proportion, aggregates) = family_facts(
            state,
            cells_row(&input.catalog, &input.database, &input.cells),
        )?;
        Ok(SimilarSqlMember {
            member_key: input.member_key,
            source: input.source,
            catalog: input.catalog,
            database: input.database,
            template: member.template,
            structure: member.structure,
            detached: member.detached,
            execution_count: execution_count.ok_or_else(|| invalid_response(state))?,
            error_count: error_count.ok_or_else(|| invalid_response(state))?,
            error_proportion,
            aggregates,
        })
    };
    let clustered: usize = out
        .clusters
        .iter()
        .map(|cluster| cluster.members.len())
        .sum();
    let clusters = out
        .clusters
        .into_iter()
        .map(|cluster| {
            let (execution_count, error_count, error_proportion, aggregates) = family_facts(
                state,
                cells_row(&cluster.catalog, &cluster.database, &cluster.cells),
            )?;
            Ok(SimilarSqlCluster {
                cluster_key: cluster.cluster_key,
                basis: cluster.basis,
                refined: cluster.refined,
                members: cluster
                    .members
                    .into_iter()
                    .map(member)
                    .collect::<Result<_, ApiFailure>>()?,
                catalog: cluster.catalog,
                database: cluster.database,
                execution_count: execution_count.ok_or_else(|| invalid_response(state))?,
                error_count: error_count.ok_or_else(|| invalid_response(state))?,
                error_proportion,
                aggregates,
            })
        })
        .collect::<Result<Vec<_>, ApiFailure>>()?;
    let independent_count = out.independent.len();
    let unavailable_count = out.unavailable.len();
    let exact_families = if top.truncated {
        SimilarSqlFamilySource::Partial {
            read: top.returned_count,
            in_window: top.full_window_count.clone(),
        }
    } else {
        SimilarSqlFamilySource::Complete
    };
    Ok(AuditSimilarSqlResponse {
        schema_version: AuditWorkloadVersion::V1,
        connection_id: identity.connection_id.clone(),
        scope,
        cluster_identity: public_identity(&identity),
        audit_timestamp_time_zone: public_time_zone(&identity),
        structure_version: SIMILAR_SQL_STRUCTURE_VERSION.into(),
        dimensions: request.dimensions.clone(),
        clusters,
        independent: out
            .independent
            .into_iter()
            .map(member)
            .collect::<Result<_, ApiFailure>>()?,
        unavailable: out
            .unavailable
            .into_iter()
            .map(member)
            .collect::<Result<_, ApiFailure>>()?,
        top,
        coverage: SimilarSqlCoverage {
            exact_families,
            fingerprint,
            member_count: member_count.to_string(),
            clustered_member_count: clustered.to_string(),
            independent_member_count: independent_count.to_string(),
            unavailable_member_count: unavailable_count.to_string(),
        },
        limitations: [
            "similar_sql_basis_is_a_product_rule_not_starrocks_semantics",
            "structure_reads_plain_select_only_and_marks_other_shapes_unavailable",
            "official_family_template_is_normalized_from_its_most_recent_member",
            "exact_families_are_one_page_by_the_requested_sort",
            "detach_and_refine_apply_to_this_request_only",
            "cluster_identity_is_session_scoped_not_cross_connection",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    })
}

fn session_context(
    identity: &AuditSessionIdentity,
    catalog: &str,
    database: &str,
) -> AuditFamilyContext {
    AuditFamilyContext {
        cluster: Some(format!("session:{}", identity.connection_id)),
        engine_version: Some(identity.detected_version.clone()),
        catalog: Some(catalog.to_owned()),
        database: Some(database.to_owned()),
    }
}

/// In-process cells in the exact-family SQL row shape, so both are projected
/// by the same typed-fact code.
fn cells_row(catalog: &str, database: &str, cells: &FamilyCells) -> AuditFamilySourceRow {
    let cell = |value: Option<u128>| value.map(|value| value.to_string());
    AuditFamilySourceRow {
        catalog: Some(catalog.to_owned()),
        database: Some(database.to_owned()),
        native_digest: None,
        canonical_execution_count: Some(cells.execution_count.to_string()),
        error_count: Some(cells.error_count.to_string()),
        query_time_valid_count: Some(cells.query_time.valid_count.to_string()),
        cumulative_query_time: cell(cells.query_time.sum),
        scan_bytes_valid_count: Some(cells.scan_bytes.valid_count.to_string()),
        cumulative_scan_bytes: cell(cells.scan_bytes.sum),
        cpu_cost_valid_count: Some(cells.cpu_cost.valid_count.to_string()),
        cumulative_cpu_cost: cell(cells.cpu_cost.sum),
        memory_cost_valid_count: Some(cells.memory_cost.valid_count.to_string()),
        max_memory_cost: cell(cells.memory_cost.max),
        max_query_time: cell(cells.query_time.max),
        error_proportion_population_max: None,
        statement_sample: AuditSourceStatementSample::NotRead,
    }
}

/// Groups the digest-missing executions and states the read's coverage.
fn grouped_fingerprints(
    state: &ApiState,
    workload: AuditFingerprintWorkload,
) -> Result<(Vec<AuditFingerprintGroup>, AuditFingerprintCoverage), ApiFailure> {
    let read_event_count = workload.rows.len();
    let events = workload
        .rows
        .into_iter()
        .map(|row| {
            let (Some(catalog), Some(database)) = (row.catalog, row.database) else {
                return Err(invalid_response(state));
            };
            Ok(AuditFingerprintEvent {
                catalog,
                database,
                statement: match row.statement_sample {
                    AuditSourceStatementSample::Read(AuditStatementSampleSource::Text {
                        value,
                        truncated,
                    }) => AuditFingerprintStatement::Text { value, truncated },
                    AuditSourceStatementSample::Read(AuditStatementSampleSource::SqlNull) => {
                        AuditFingerprintStatement::SqlNull
                    }
                    AuditSourceStatementSample::ConflictingSourceRows => {
                        AuditFingerprintStatement::Conflicting
                    }
                    AuditSourceStatementSample::NotRead
                    | AuditSourceStatementSample::SourceColumnAbsent => {
                        AuditFingerprintStatement::NotRead
                    }
                },
                error: row.state.as_deref() == Some("ERR"),
                metrics: row.metrics,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (groups, tally) = group_audit_fingerprints(&events);
    let eligible =
        canonical_count(&workload.eligible_count).ok_or_else(|| invalid_response(state))?;
    let read_text = read_event_count.to_string();
    let source = if (eligible.len(), eligible) > (read_text.len(), read_text.as_str()) {
        AuditFingerprintSourceCoverage::Partial {
            reason: AuditFingerprintPartialReason::ReadLimit,
        }
    } else {
        AuditFingerprintSourceCoverage::Complete
    };
    let coverage = AuditFingerprintCoverage {
        eligible_event_count: eligible.to_owned(),
        read_limit: AUDIT_FINGERPRINT_READ_LIMIT,
        read_event_count: read_text,
        source,
        fingerprinted_event_count: tally.fingerprinted.to_string(),
        partial_event_count: tally.partial.to_string(),
        unavailable: tally
            .unavailable
            .into_iter()
            .map(|(reason, count)| AuditFingerprintUnavailableCount {
                reason,
                count: count.to_string(),
            })
            .collect(),
        family_count: groups.len().to_string(),
    };
    Ok((groups, coverage))
}

pub(super) async fn family_detail(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: AuditFamilyDetailReadRequest = connector_json_request(&state, request).await?;
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    validate_audit_scope_window(&state, &request.scope)?;
    let lease = begin_read(&state)?;
    audit_connector_call(
        &state,
        lease,
        connection_id.clone(),
        move |sessions, control, state| {
            let prepared = sessions
                .prepare_audit_family_detail_controlled(&connection_id, request, Some(control))
                .map_err(|error| connector_failure(state, error))?;
            // Finish every fallible projection before publishing a new discovery.
            let mut response = family_detail_response(state, prepared.workload())?;
            let generation = sessions
                .commit_audit_family_detail_controlled(prepared, Some(control))
                .map_err(|error| connector_failure(state, error))?;
            response.discovery_generation = generation;
            for representative in [
                &mut response.representatives.recent,
                &mut response.representatives.slowest,
                &mut response.representatives.profile_known,
            ] {
                if let AuditFamilyRepresentative::Selected { occurrence, .. } = representative {
                    occurrence.query.discovery_generation = Some(generation);
                }
            }
            Ok(no_store(response))
        },
    )
    .await
}

pub(super) fn family_detail_response(
    state: &ApiState,
    workload: &AuditFamilyDetailWorkload,
) -> Result<AuditFamilyDetailResponse, ApiFailure> {
    let identity = &workload.session_identity;
    let time_zone = public_time_zone(identity).ok_or_else(|| invalid_response(state))?;
    let AuditFamilyExecutionDistribution::Exact {
        population_count, ..
    } = &workload.source.distribution;
    let empty = population_count == "0";
    let project =
        |row: &Option<AuditOccurrenceSourceRow>, evidence: Option<AuditProfileEvidence>, reason| {
            let Some(row) = row else {
                return Ok(AuditFamilyRepresentative::Unavailable { reason });
            };
            let id = row
                .query_id
                .as_deref()
                .and_then(canonical_query_id)
                .ok_or_else(|| invalid_response(state))?;
            let index = workload
                .representative_indices
                .get(&id)
                .copied()
                .ok_or_else(|| invalid_response(state))?;
            Ok(AuditFamilyRepresentative::Selected {
                occurrence: Box::new(occurrence_row(
                    state,
                    identity,
                    workload.discovery_generation,
                    index,
                    row.clone(),
                )?),
                profile_evidence: evidence,
            })
        };
    if (!empty && workload.source.recent.is_none())
        || workload.source.profile_known.is_some() != workload.selected_profile_evidence.is_some()
    {
        return Err(invalid_response(state));
    }
    use AuditFamilyRepresentativeUnavailableReason::{
        EmptyCohort, NoEligibleQueryTime, NoMatchInBoundedProfileEvidence,
    };
    Ok(AuditFamilyDetailResponse {
        schema_version: AuditWorkloadVersion::V1,
        connection_id: identity.connection_id.clone(),
        discovery_generation: workload.discovery_generation,
        scope: workload.scope.clone(),
        family: workload.family.clone(),
        distribution: workload.source.distribution.clone(),
        representatives: AuditFamilyRepresentatives {
            recent: project(&workload.source.recent, None, EmptyCohort)?,
            slowest: project(
                &workload.source.slowest,
                None,
                if empty {
                    EmptyCohort
                } else {
                    NoEligibleQueryTime
                },
            )?,
            profile_known: project(
                &workload.source.profile_known,
                workload.selected_profile_evidence.clone(),
                if empty {
                    EmptyCohort
                } else {
                    NoMatchInBoundedProfileEvidence
                },
            )?,
        },
        profile_evidence_coverage: workload.profile_evidence_coverage.clone(),
        cluster_identity: public_identity(identity),
        audit_timestamp_time_zone: time_zone,
        limitations: [
            "audit_loader_ingestion_async_completeness_unknown",
            "canonical_cohort_excludes_invalid_query_ids_and_conflicting_base_projections",
            "exact_native_digest_is_not_semantic_sql_clustering",
            "cluster_identity_is_session_scoped_not_cross_connection",
            "audit_timestamp_utc_is_reader_session_interpretation_not_plugin_epoch",
            "profile_evidence_and_audit_detail_are_non_atomic_reads",
            "no_match_in_bounded_profile_evidence_does_not_prove_profile_absence",
            "profile_cache_availability_is_observed_at_fetch_time",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    })
}

pub(super) async fn overview(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: AuditOverviewReadRequest = connector_json_request(&state, request).await?;
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    validate_audit_scope_window(&state, &request.request.scope)?;
    let lease = begin_read(&state)?;
    audit_connector_call(
        &state,
        lease,
        connection_id.clone(),
        move |sessions, control, state| {
            let workload = sessions
                .audit_overview_controlled(&connection_id, request.request, Some(control))
                .map_err(|error| connector_failure(state, error))?;
            Ok(no_store(overview_response(state, workload)?))
        },
    )
    .await
}

pub(super) async fn sql(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: AuditSqlReadRequest = connector_json_request(&state, request).await?;
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    validate_audit_scope_window(&state, &request.scope)?;
    let lease = begin_read(&state)?;
    audit_connector_call(
        &state,
        lease,
        connection_id.clone(),
        move |sessions, control, state| {
            let query = sessions
                .audit_query_sql_controlled(
                    &connection_id,
                    AuditSqlLookupRequest {
                        scope: request.scope,
                        query_id: request.query_id,
                    },
                    request.discovery_generation,
                    Some(control),
                )
                .map_err(|error| connector_failure(state, error))?;
            Ok(no_store(ConnectedQuerySqlResponse {
                query_id: query.query_id,
                discovery_generation: query.discovery_generation,
                sql: query.sql,
            }))
        },
    )
    .await
}

pub(super) async fn active_at(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: AuditActiveAtReadRequest = connector_json_request(&state, request).await?;
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    validate_audit_scope_window(&state, &request.scope)?;
    let lease = begin_read(&state)?;
    audit_connector_call(
        &state,
        lease,
        connection_id.clone(),
        move |sessions, control, state| {
            let workload = sessions
                .audit_active_at_controlled(
                    &connection_id,
                    request.scope,
                    request.at_milliseconds,
                    request.discovery_generation,
                    Some(control),
                )
                .map_err(|error| connector_failure(state, error))?;
            Ok(no_store(active_at_response(state, workload)?))
        },
    )
    .await
}

pub(crate) fn no_store<T: Serialize>(value: T) -> Response {
    let mut response = Json(value).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn begin_read(state: &ApiState) -> Result<WorkLease, ApiFailure> {
    state
        .budgets
        .begin(WorkKind::Analysis)
        .map_err(|v| v.failure(state.next_request_id()))
}

/// Enforce the shared Audit scope ceiling before a lease or remote session is
/// touched. Other malformed scope fields retain their existing typed connector
/// validation path; the user-visible upper-window boundary is explicitly 422.
fn validate_audit_scope_window(state: &ApiState, scope: &AuditScope) -> Result<(), ApiFailure> {
    if matches!(
        scope.validate(),
        Err(AuditWorkloadValidationError::WindowTooWide)
    ) {
        return Err(ApiFailure::new(
            state.next_request_id(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "audit_window_too_wide",
            "audit_scope",
            "audit_window_too_wide",
            "change_input",
        ));
    }
    Ok(())
}

fn occurrence_response(
    state: &ApiState,
    workload: AuditOccurrenceWorkload,
    limit: u16,
) -> Result<AuditOccurrenceResponse, ApiFailure> {
    let coverage = public_coverage(workload.coverage);
    let top = top_window(
        state,
        limit,
        workload.rows.len(),
        &coverage.retained_event_count,
    )?;
    let rows = workload
        .rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            occurrence_row(
                state,
                &workload.session_identity,
                workload.discovery_generation,
                index,
                row,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AuditOccurrenceResponse {
        schema_version: AuditWorkloadVersion::V1,
        connection_id: workload.session_identity.connection_id.clone(),
        discovery_generation: workload.discovery_generation,
        scope: workload.scope,
        cluster_identity: public_identity(&workload.session_identity),
        audit_timestamp_time_zone: public_time_zone(&workload.session_identity),
        rows,
        coverage,
        top,
        limitations: limitations(&workload.session_identity),
    })
}

fn family_response(
    state: &ApiState,
    workload: AuditFamilyWorkload,
    limit: u16,
) -> Result<AuditFamilyResponse, ApiFailure> {
    let coverage = public_coverage(workload.coverage);
    let top = top_window(
        state,
        limit,
        workload.rows.len(),
        &coverage.retained_family_count,
    )?;
    let rows = workload
        .rows
        .into_iter()
        .map(|row| family_row(state, &workload.session_identity, row))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AuditFamilyResponse {
        schema_version: AuditWorkloadVersion::V1,
        connection_id: workload.session_identity.connection_id.clone(),
        discovery_generation: workload.discovery_generation,
        scope: workload.scope,
        cluster_identity: public_identity(&workload.session_identity),
        audit_timestamp_time_zone: public_time_zone(&workload.session_identity),
        rows,
        coverage,
        top,
        limitations: limitations(&workload.session_identity),
    })
}

fn overview_response(
    state: &ApiState,
    workload: AuditOverviewWorkload,
) -> Result<AuditOverviewResponse, ApiFailure> {
    let coverage = public_coverage(workload.coverage);
    let identity = &workload.session_identity;
    let primary = match workload.primary {
        AuditOverviewPrimaryWorkload::Occurrences { rows, limit } => {
            AuditOverviewPrimaryPresentation::Occurrences {
                top: top_window(state, limit, rows.len(), &coverage.retained_event_count)?,
                rows: rows
                    .into_iter()
                    .enumerate()
                    .map(|(index, row)| {
                        occurrence_row(state, identity, workload.discovery_generation, index, row)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            }
        }
        AuditOverviewPrimaryWorkload::Families { rows, limit } => {
            AuditOverviewPrimaryPresentation::Families {
                top: top_window(state, limit, rows.len(), &coverage.retained_family_count)?,
                rows: rows
                    .into_iter()
                    .map(|row| family_row(state, identity, row))
                    .collect::<Result<Vec<_>, _>>()?,
            }
        }
    };
    let concurrency = concurrency_presentation(state, &workload.scope, workload.concurrency)?;
    let family_rows = workload
        .family_top_rows
        .into_iter()
        .map(|row| family_row(state, identity, row))
        .collect::<Result<Vec<_>, _>>()?;
    let family_top = overview_family_top(
        state,
        workload.family_top_sort,
        family_rows,
        &coverage.retained_family_count,
    )?;
    let mut overview_limitations = limitations(identity);
    overview_limitations.push(
        "overview_coverage_primary_concurrency_and_family_top_are_separate_non_atomic_reads".into(),
    );
    Ok(AuditOverviewResponse {
        schema_version: AuditWorkloadVersion::V1,
        connection_id: identity.connection_id.clone(),
        discovery_generation: workload.discovery_generation,
        scope: workload.scope,
        cluster_identity: public_identity(identity),
        audit_timestamp_time_zone: public_time_zone(identity),
        primary,
        coverage,
        concurrency,
        family_top,
        limitations: overview_limitations,
    })
}

fn concurrency_presentation(
    state: &ApiState,
    scope: &doctor_contracts::AuditScope,
    source: AuditConcurrencySource,
) -> Result<AuditConcurrencyPresentation, ApiFailure> {
    let source_coverage = bounded_source_coverage(scope);
    let window = scope.window.clone();
    let start_milliseconds = i128::from(window.start_seconds)
        .checked_mul(1_000)
        .ok_or_else(|| invalid_response(state))?;
    let end_milliseconds = i128::from(window.end_seconds)
        .checked_mul(1_000)
        .ok_or_else(|| invalid_response(state))?;
    if start_milliseconds >= end_milliseconds {
        return Err(invalid_response(state));
    }
    let window = AuditConcurrencyWindow {
        start_milliseconds: start_milliseconds.to_string(),
        end_milliseconds: end_milliseconds.to_string(),
    };
    match source {
        AuditConcurrencySource::SchemaTypeUnsupported => Ok(AuditConcurrencyPresentation {
            window,
            axis: AuditConcurrencyAxis { time_ticks: vec![] },
            source_coverage,
            coverage: None,
            series: AuditConcurrencySeries::Unavailable {
                reason: AuditConcurrencyUnavailableReason::SchemaTypeUnsupported,
            },
        }),
        AuditConcurrencySource::TransitionPointBudgetExceeded {
            coverage,
            axis_labels,
        } => Ok(AuditConcurrencyPresentation {
            window,
            axis: concurrency_axis(state, start_milliseconds, end_milliseconds, axis_labels)?,
            source_coverage,
            coverage: Some(concurrency_coverage(coverage)),
            series: AuditConcurrencySeries::Unavailable {
                reason: AuditConcurrencyUnavailableReason::TransitionPointBudgetExceeded,
            },
        }),
        AuditConcurrencySource::Exact {
            coverage,
            axis_labels,
            deltas,
        } => {
            let axis = concurrency_axis(state, start_milliseconds, end_milliseconds, axis_labels)?;
            let initial_active_count = exact_i128(state, &coverage.initial_active_count)?;
            let core_deltas = deltas
                .iter()
                .map(|delta| AuditConcurrencyDelta {
                    at_ms: delta.at_milliseconds,
                    delta: delta.delta,
                })
                .collect::<Vec<_>>();
            let steps = audit_concurrency_from_deltas(
                start_milliseconds,
                end_milliseconds,
                initial_active_count,
                &core_deltas,
                AUDIT_CONCURRENCY_MAX_STEPS,
            )
            .map_err(|_| invalid_response(state))?;
            let maximum = steps
                .iter()
                .map(|step| step.active_count)
                .max()
                .ok_or_else(|| invalid_response(state))?;
            let points = steps
                .iter()
                .enumerate()
                .map(|(index, step)| {
                    Ok(AuditConcurrencyPoint {
                        at_milliseconds: step.at_ms.to_string(),
                        active_count: step.active_count.to_string(),
                        x_permyriad: permyriad_i128(
                            state,
                            step.at_ms
                                .checked_sub(start_milliseconds)
                                .ok_or_else(|| invalid_response(state))?,
                            end_milliseconds - start_milliseconds,
                        )?,
                        y_permyriad: permyriad_i128(state, step.active_count, maximum)?,
                        label: if index == 0 {
                            axis.time_ticks
                                .first()
                                .map(|tick| tick.label.clone())
                                .ok_or_else(|| invalid_response(state))?
                        } else {
                            deltas
                                .get(index - 1)
                                .map(|delta| delta.label.clone())
                                .ok_or_else(|| invalid_response(state))?
                        },
                    })
                })
                .collect::<Result<Vec<_>, ApiFailure>>()?;
            let count_ticks = if maximum == 0 {
                vec![AuditConcurrencyCountTick {
                    active_count: "0".to_owned(),
                    y_permyriad: 0,
                }]
            } else {
                vec![
                    AuditConcurrencyCountTick {
                        active_count: "0".to_owned(),
                        y_permyriad: 0,
                    },
                    AuditConcurrencyCountTick {
                        active_count: maximum.to_string(),
                        y_permyriad: 10_000,
                    },
                ]
            };
            Ok(AuditConcurrencyPresentation {
                window,
                axis,
                source_coverage,
                coverage: Some(concurrency_coverage(coverage)),
                series: AuditConcurrencySeries::Exact {
                    maximum_active_count: maximum.to_string(),
                    count_ticks,
                    points,
                },
            })
        }
    }
}

fn concurrency_axis(
    state: &ApiState,
    start_milliseconds: i128,
    end_milliseconds: i128,
    labels: Vec<String>,
) -> Result<AuditConcurrencyAxis, ApiFailure> {
    if labels.len() != 5 {
        return Err(invalid_response(state));
    }
    let duration = end_milliseconds - start_milliseconds;
    let time_ticks = labels
        .into_iter()
        .enumerate()
        .map(|(index, label)| {
            let numerator = i128::try_from(index)
                .map_err(|_| invalid_response(state))?
                .checked_mul(duration)
                .ok_or_else(|| invalid_response(state))?;
            let at_milliseconds = start_milliseconds
                .checked_add(numerator / 4)
                .ok_or_else(|| invalid_response(state))?;
            Ok(AuditConcurrencyTimeTick {
                at_milliseconds: at_milliseconds.to_string(),
                x_permyriad: u16::try_from(index * 2_500).map_err(|_| invalid_response(state))?,
                label,
            })
        })
        .collect::<Result<Vec<_>, ApiFailure>>()?;
    Ok(AuditConcurrencyAxis { time_ticks })
}

fn concurrency_coverage(
    source: doctor_connector::AuditConcurrencyCoverageSource,
) -> AuditConcurrencyCoverage {
    AuditConcurrencyCoverage {
        canonical_bounded_event_count: source.canonical_bounded_event_count,
        interval_count: source.interval_count,
        initial_active_count: source.initial_active_count,
        transition_count: source.transition_count,
        timestamp_missing_count: source.timestamp_missing_count,
        timestamp_invalid_count: source.timestamp_invalid_count,
        non_terminal_count: source.non_terminal_count,
        query_time_sql_null_count: source.query_time_sql_null_count,
        query_time_negative_count: source.query_time_negative_count,
        query_time_overflow_count: source.query_time_overflow_count,
        zero_duration_count: source.zero_duration_count,
        endpoint_overflow_count: source.endpoint_overflow_count,
    }
}

fn bounded_source_coverage(scope: &doctor_contracts::AuditScope) -> AuditBoundedSourceCoverage {
    let source = audit_historical_source_window(scope);
    AuditBoundedSourceCoverage::Partial {
        reason: AuditBoundedSourcePartialReason::BoundedLookback,
        source_start_seconds: source.start_seconds,
        source_end_seconds: source.end_seconds,
        lookback_seconds: AUDIT_HISTORICAL_LOOKBACK_SECONDS,
    }
}

fn exact_i128(state: &ApiState, value: &str) -> Result<i128, ApiFailure> {
    canonical_count(value)
        .and_then(|value| value.parse::<i128>().ok())
        .ok_or_else(|| invalid_response(state))
}

fn permyriad_i128(state: &ApiState, value: i128, maximum: i128) -> Result<u16, ApiFailure> {
    if value < 0 || maximum < 0 || value > maximum {
        return Err(invalid_response(state));
    }
    if maximum == 0 {
        return Ok(0);
    }
    let projected = value
        .checked_mul(10_000)
        .ok_or_else(|| invalid_response(state))?
        / maximum;
    u16::try_from(projected.min(10_000)).map_err(|_| invalid_response(state))
}

fn active_at_response(
    state: &ApiState,
    workload: AuditActiveAtWorkload,
) -> Result<AuditActiveAtResponse, ApiFailure> {
    let identity = &workload.session_identity;
    let active_count = canonical_count(&workload.active_count)
        .map(str::to_owned)
        .ok_or_else(|| invalid_response(state))?;
    let rows = workload
        .rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            Ok(AuditActiveAtParticipant {
                occurrence: occurrence_row(
                    state,
                    identity,
                    workload.discovery_generation,
                    index,
                    row.occurrence,
                )?,
                start_milliseconds: row.start_milliseconds.to_string(),
                end_milliseconds: row.end_milliseconds.to_string(),
                elapsed_milliseconds: row.elapsed_milliseconds.to_string(),
            })
        })
        .collect::<Result<Vec<_>, ApiFailure>>()?;
    let returned_count = rows.len().to_string();
    if compare_decimal(&active_count, &returned_count).is_lt() {
        return Err(invalid_response(state));
    }
    let participants = if workload.truncated {
        AuditActiveAtParticipants::Truncated {
            omitted_count: decimal_subtract(&active_count, &returned_count),
            rows,
        }
    } else if active_count == returned_count {
        AuditActiveAtParticipants::Exact { rows }
    } else {
        return Err(invalid_response(state));
    };
    let mut response_limitations = limitations(identity);
    response_limitations
        .push("historical_terminal_intervals_are_approximate_not_live_or_blocking".to_owned());
    response_limitations
        .push("active_at_participants_are_deterministic_bounded_not_a_full_sql_list".to_owned());
    let source_coverage = bounded_source_coverage(&workload.scope);
    Ok(AuditActiveAtResponse {
        schema_version: AuditWorkloadVersion::V1,
        connection_id: identity.connection_id.clone(),
        discovery_generation: workload.discovery_generation,
        scope: workload.scope,
        at_milliseconds: workload.at_milliseconds.to_string(),
        source_coverage,
        active_count,
        participants,
        cluster_identity: public_identity(identity),
        audit_timestamp_time_zone: public_time_zone(identity),
        limitations: response_limitations,
    })
}

fn overview_family_top(
    state: &ApiState,
    sort: doctor_contracts::AuditOverviewFamilyTopSort,
    rows: Vec<AuditFamilyPresentation>,
    total: &str,
) -> Result<AuditOverviewFamilyTopPresentation, ApiFailure> {
    let values = rows
        .iter()
        .filter_map(|family| family_sort_value(family, sort))
        .collect::<Vec<_>>();
    let maximum = decimal_max(values.into_iter()).map(str::to_owned);
    let top = top_window(state, AUDIT_OVERVIEW_FAMILY_TOP_LIMIT, rows.len(), total)?;
    Ok(AuditOverviewFamilyTopPresentation {
        sort,
        projection_scope: AuditOverviewProjectionScope::ReturnedTopRows,
        rows: rows
            .into_iter()
            .map(|family| AuditOverviewFamilyTopRow {
                plot: family_sort_value(&family, sort).map_or_else(unavailable_plot, |value| {
                    plot_decimal(Some(value), maximum.as_deref())
                }),
                family,
            })
            .collect(),
        top,
    })
}

fn family_sort_value(
    family: &AuditFamilyPresentation,
    sort: doctor_contracts::AuditOverviewFamilyTopSort,
) -> Option<&str> {
    match sort {
        doctor_contracts::AuditOverviewFamilyTopSort::ExecutionCountDesc => {
            family.execution_count.as_deref()
        }
        doctor_contracts::AuditOverviewFamilyTopSort::CumulativeQueryTimeDesc => family
            .aggregates
            .iter()
            .find(|fact| {
                fact.aggregation == AuditAggregation::Sum
                    && fact.metric.metric_id == AuditMetricId::QueryTime
            })
            .and_then(|fact| canonical_metric_value(&fact.metric)),
        doctor_contracts::AuditOverviewFamilyTopSort::CumulativeScanBytesDesc => family
            .aggregates
            .iter()
            .find(|fact| {
                fact.aggregation == AuditAggregation::Sum
                    && fact.metric.metric_id == AuditMetricId::ScanBytes
            })
            .and_then(|fact| canonical_metric_value(&fact.metric)),
        doctor_contracts::AuditOverviewFamilyTopSort::CumulativeCpuCostDesc => family
            .aggregates
            .iter()
            .find(|fact| {
                fact.aggregation == AuditAggregation::Sum
                    && fact.metric.metric_id == AuditMetricId::CpuCost
            })
            .and_then(|fact| canonical_metric_value(&fact.metric)),
    }
}

fn canonical_metric_value(metric: &doctor_contracts::AuditMetricFact) -> Option<&str> {
    matches!(
        metric.state,
        AuditMetricState::Exact | AuditMetricState::PrecisionUnsafe
    )
    .then(|| {
        metric
            .canonical_number
            .as_ref()
            .map(|number| number.value.as_str())
    })
    .flatten()
    .filter(|value| canonical_count(value).is_some())
}
fn unavailable_plot() -> AuditPlotProjection {
    AuditPlotProjection::Unavailable {
        reason: AuditPlotUnavailableReason::MetricNotPlottable,
    }
}
fn decimal_max<'a>(values: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    values
        .filter_map(canonical_count)
        .max_by(|left, right| (left.len(), *left).cmp(&(right.len(), *right)))
}
fn plot_decimal(value: Option<&str>, maximum: Option<&str>) -> AuditPlotProjection {
    match (
        value.and_then(canonical_count),
        maximum.and_then(canonical_count),
    ) {
        (_, None) | (_, Some("0")) => AuditPlotProjection::NotApplicable {
            reason: AuditPlotNotApplicableReason::ZeroSeriesMaximum,
        },
        (Some(value), Some(maximum)) => AuditPlotProjection::Exact {
            permyriad: decimal_permyriad(value, maximum),
        },
        (None, Some(_)) => unavailable_plot(),
    }
}
fn decimal_permyriad(value: &str, maximum: &str) -> u16 {
    if value == maximum {
        return 10_000;
    }
    let quotient = decimal_divide_floor(&format!("{value}0000"), maximum);
    quotient.parse::<u16>().unwrap_or(10_000).min(10_000)
}
fn decimal_divide_floor(numerator: &str, divisor: &str) -> String {
    let mut remainder = "0".to_owned();
    let mut output = String::new();
    for digit in numerator.bytes() {
        remainder = canonical_decimal_owned(format!("{remainder}{}", digit as char));
        let mut count = 0u8;
        while compare_decimal(&remainder, divisor).is_ge() {
            remainder = decimal_subtract(&remainder, divisor);
            count += 1;
        }
        output.push((b'0' + count) as char);
    }
    canonical_decimal_owned(output)
}
fn canonical_decimal_owned(value: String) -> String {
    let trimmed = value.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".into()
    } else {
        trimmed.into()
    }
}
fn compare_decimal(left: &str, right: &str) -> std::cmp::Ordering {
    (left.len(), left).cmp(&(right.len(), right))
}
fn decimal_subtract(left: &str, right: &str) -> String {
    let mut borrow = 0i8;
    let mut result = Vec::new();
    let left = left.bytes().rev();
    let mut right = right.bytes().rev();
    for a in left {
        let mut digit =
            a as i8 - b'0' as i8 - borrow - right.next().map_or(0, |b| b as i8 - b'0' as i8);
        if digit < 0 {
            digit += 10;
            borrow = 1;
        } else {
            borrow = 0;
        }
        result.push((digit as u8 + b'0') as char);
    }
    canonical_decimal_owned(result.into_iter().rev().collect())
}

fn occurrence_row(
    state: &ApiState,
    identity: &AuditSessionIdentity,
    generation: u32,
    row_index: usize,
    row: AuditOccurrenceSourceRow,
) -> Result<AuditOccurrencePresentation, ApiFailure> {
    // The connector's canonical set must have a unique valid ID. Do not drop
    // a bad public row silently and manufacture a different returned count.
    let query_id = row
        .query_id
        .as_deref()
        .filter(|id| is_valid_query_id(id))
        .and_then(canonical_query_id)
        .ok_or_else(|| invalid_response(state))?;
    let observed_at_local = row
        .event_time
        .as_deref()
        .and_then(|value| bounded_text(value, 64));
    let observed_at_utc = identity.audit_timestamp_time_zone.as_ref().and_then(|_| {
        row.event_epoch_seconds
            .as_deref()
            .and_then(unix_seconds_rfc3339)
    });
    let status = match row.state.as_deref() {
        Some(value) if value.eq_ignore_ascii_case("EOF") => "finished",
        Some(value) if value.eq_ignore_ascii_case("ERR") => "error",
        _ => "unknown",
    }
    .to_owned();
    let query = ConnectedQueryRef {
        connection_id: identity.connection_id.clone(),
        profile_fetch: profile_fetch_action(&query_id, &status, false),
        plan_check: plan_check_action(&query_id, None, false),
        query_id,
        discovery_source: Some(QueryDiscoverySource::Audit),
        discovery_generation: Some(generation),
        timestamp_state: if observed_at_utc.is_some() {
            ConnectedTimestampState::ReaderSessionInterpreted
        } else if observed_at_local.is_some() {
            ConnectedTimestampState::RawWallTimeOnly
        } else {
            ConnectedTimestampState::Unavailable
        },
        observed_at_utc,
        observed_at_local,
        status,
        digest: row.native_digest.clone(),
        database: row.database.clone(),
        resource_group: row.resource_group,
        // Kept empty rather than narrowing the new lossless audit fact to the
        // legacy u32 field. The audit UI must consume `metrics` below.
        query_time_ms: None,
        source_refs: vec![
            format!("audit:row:{row_index}:timestamp"),
            format!("audit:row:{row_index}:epoch"),
        ],
    };
    let family = membership(
        identity,
        row.catalog.clone(),
        row.database,
        row.native_digest.as_deref(),
    );
    Ok(AuditOccurrencePresentation {
        query,
        catalog: row.catalog,
        metrics: row
            .metrics
            .into_iter()
            .map(|(id, input)| normalize_audit_metric(id, input))
            .collect(),
        error_code: match row.error_code {
            AuditErrorCodeSource::Unavailable { reason } => {
                AuditErrorCodeFact::Unavailable { reason }
            }
            AuditErrorCodeSource::SqlNull => AuditErrorCodeFact::SqlNull,
            AuditErrorCodeSource::Text { value } => AuditErrorCodeFact::Text { value },
        },
        family,
        statement_sample: statement_sample(row.statement_sample),
    })
}

fn family_row(
    state: &ApiState,
    identity: &AuditSessionIdentity,
    row: AuditFamilySourceRow,
) -> Result<AuditFamilyPresentation, ApiFailure> {
    let membership = membership(
        identity,
        row.catalog.clone(),
        row.database.clone(),
        row.native_digest.as_deref(),
    );
    if !matches!(membership, AuditFamilyMembership::ExactNativeDigest { .. }) {
        return Err(invalid_response(state));
    }
    let statement_sample = statement_sample(row.statement_sample.clone());
    let (execution_count, error_count, error_proportion, aggregates) = family_facts(state, row)?;
    Ok(AuditFamilyPresentation {
        membership,
        execution_count,
        error_count,
        error_proportion,
        aggregates,
        statement_sample,
    })
}

type FamilyFacts = (
    Option<String>,
    Option<String>,
    AuditErrorProportion,
    Vec<AuditAggregateFact>,
);

/// Counters and aggregates of one family row, from SQL or in-process cells.
fn family_facts(state: &ApiState, row: AuditFamilySourceRow) -> Result<FamilyFacts, ApiFailure> {
    let aggregate = |id, value, count, aggregation| -> Result<_, ApiFailure> {
        Ok(AuditAggregateFact {
            aggregation,
            metric: normalize_audit_metric(id, AuditMetricInput::from_sql_cell(value)),
            source_nonnegative_count: optional_count(state, count)?,
        })
    };
    Ok((
        family_count(row.canonical_execution_count.clone()),
        family_count(row.error_count.clone()),
        error_proportion(
            row.error_count.as_deref(),
            row.canonical_execution_count.as_deref(),
        ),
        vec![
            aggregate(
                AuditMetricId::QueryTime,
                row.cumulative_query_time,
                row.query_time_valid_count.clone(),
                AuditAggregation::Sum,
            )?,
            aggregate(
                AuditMetricId::ScanBytes,
                row.cumulative_scan_bytes,
                row.scan_bytes_valid_count,
                AuditAggregation::Sum,
            )?,
            aggregate(
                AuditMetricId::CpuCost,
                row.cumulative_cpu_cost,
                row.cpu_cost_valid_count,
                AuditAggregation::Sum,
            )?,
            aggregate(
                AuditMetricId::MemoryCost,
                row.max_memory_cost,
                row.memory_cost_valid_count,
                AuditAggregation::Max,
            )?,
            aggregate(
                AuditMetricId::QueryTime,
                row.max_query_time,
                row.query_time_valid_count,
                AuditAggregation::Max,
            )?,
        ],
    ))
}

fn statement_sample(sample: AuditSourceStatementSample) -> AuditStatementSample {
    let unavailable = |reason| AuditStatementSample::Unavailable { reason };
    match sample {
        AuditSourceStatementSample::NotRead => {
            unavailable(AuditStatementSampleUnavailableReason::NotRead)
        }
        AuditSourceStatementSample::SourceColumnAbsent => {
            unavailable(AuditStatementSampleUnavailableReason::SourceColumnAbsent)
        }
        AuditSourceStatementSample::ConflictingSourceRows => {
            unavailable(AuditStatementSampleUnavailableReason::ConflictingSourceRows)
        }
        AuditSourceStatementSample::Read(AuditStatementSampleSource::SqlNull) => {
            AuditStatementSample::SqlNull
        }
        AuditSourceStatementSample::Read(AuditStatementSampleSource::Text { value, truncated }) => {
            AuditStatementSample::Text { value, truncated }
        }
    }
}

fn public_coverage(coverage: AuditWindowCoverage) -> AuditCoverage {
    let error_proportion = error_proportion(
        Some(&coverage.error_count),
        Some(&coverage.retained_event_count),
    );
    AuditCoverage {
        raw_row_count: coverage.raw_row_count,
        invalid_query_id_count: coverage.invalid_query_id_count,
        exact_duplicate_row_count: coverage.exact_duplicate_row_count,
        conflicting_duplicate_row_count: coverage.conflicting_duplicate_row_count,
        retained_event_count: coverage.retained_event_count,
        retained_family_count: coverage.retained_family_count,
        error_proportion,
        error_count: coverage.error_count,
        missing_digest_count: coverage.missing_digest_count,
        missing_context_count: coverage.missing_context_count,
        statement_filter: coverage.statement_filter,
        metric_inputs: coverage
            .metric_inputs
            .into_iter()
            .map(|input| AuditMetricCoverage {
                metric_id: input.metric_id.clone(),
                sql_null_count: input.sql_null_count,
                text_present_count: input.text_present_count,
                negative_count: input.negative_count,
                aggregate: AuditAggregateFact {
                    aggregation: match input.aggregation {
                        AuditMetricAggregation::Sum => AuditAggregation::Sum,
                        AuditMetricAggregation::Max => AuditAggregation::Max,
                    },
                    metric: normalize_audit_metric(input.metric_id, input.aggregate_input),
                    source_nonnegative_count: Some(input.nonnegative_count.clone()),
                },
                nonnegative_count: input.nonnegative_count,
            })
            .collect(),
    }
}

fn public_identity(identity: &AuditSessionIdentity) -> AuditClusterIdentity {
    AuditClusterIdentity::SessionScoped {
        connection_id: identity.connection_id.clone(),
        detected_version: identity.detected_version.clone(),
    }
}

fn public_time_zone(identity: &AuditSessionIdentity) -> Option<TimeZoneEvidence> {
    identity
        .audit_timestamp_time_zone
        .as_ref()
        .map(|evidence| TimeZoneEvidence {
            session_time_zone: evidence.session_time_zone.clone(),
            system_time_zone: evidence.system_time_zone.clone(),
            effective_time_zone: evidence.effective_time_zone.clone(),
            current_utc_offset_seconds: evidence.current_utc_offset_seconds,
            source_ref: "connection:session_time_zone".to_owned(),
        })
}

fn membership(
    identity: &AuditSessionIdentity,
    catalog: Option<String>,
    database: Option<String>,
    digest: Option<&str>,
) -> AuditFamilyMembership {
    audit_family_membership(
        digest,
        AuditFamilyContext {
            cluster: Some(format!("session:{}", identity.connection_id)),
            engine_version: Some(identity.detected_version.clone()),
            catalog,
            database,
        },
    )
}

fn limitations(identity: &AuditSessionIdentity) -> Vec<String> {
    let mut limitations = [
        "audit_loader_ingestion_async_completeness_unknown",
        "coverage_and_top_are_separate_non_atomic_reads",
        "full_window_aggregation_precedes_top_limit",
        "source_nonnegative_count_is_not_core_parser_validity_count",
        "cumulative_query_time_is_not_wall_clock_time",
        "exact_native_digest_is_not_semantic_sql_clustering",
        "cluster_identity_is_session_scoped_not_cross_connection",
        "audit_timestamp_utc_is_reader_session_interpretation_not_plugin_epoch",
        "profile_cache_availability_is_observed_at_fetch_time",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    if identity.audit_timestamp_time_zone.is_none() {
        limitations.push("audit_timestamp_time_zone_unverified".to_owned());
    }
    limitations
}

fn optional_count(state: &ApiState, count: Option<String>) -> Result<Option<String>, ApiFailure> {
    count
        .map(|value| {
            canonical_count(&value)
                .map(str::to_owned)
                .ok_or_else(|| invalid_response(state))
        })
        .transpose()
}

fn family_count(count: Option<String>) -> Option<String> {
    count.and_then(|value| canonical_count(&value).map(str::to_owned))
}

fn error_proportion(
    error_count: Option<&str>,
    execution_count: Option<&str>,
) -> AuditErrorProportion {
    let Some(numerator) = error_count else {
        return AuditErrorProportion::Unavailable {
            reason: AuditErrorProportionUnavailableReason::ErrorCountMissing,
        };
    };
    let Some(denominator) = execution_count else {
        return AuditErrorProportion::Unavailable {
            reason: AuditErrorProportionUnavailableReason::ExecutionCountMissing,
        };
    };
    let Some(numerator) = canonical_count(numerator) else {
        return AuditErrorProportion::Invalid {
            reason: AuditErrorProportionInvalidReason::ErrorCountInvalid,
        };
    };
    let Some(denominator) = canonical_count(denominator) else {
        return AuditErrorProportion::Invalid {
            reason: AuditErrorProportionInvalidReason::ExecutionCountInvalid,
        };
    };
    if numerator == "0" && denominator == "0" {
        return AuditErrorProportion::NotApplicable {
            numerator: numerator.to_owned(),
            denominator: denominator.to_owned(),
        };
    }
    if (numerator.len(), numerator) > (denominator.len(), denominator) {
        return AuditErrorProportion::Invalid {
            reason: AuditErrorProportionInvalidReason::NumeratorExceedsDenominator,
        };
    }
    AuditErrorProportion::Exact {
        numerator: numerator.to_owned(),
        denominator: denominator.to_owned(),
    }
}

fn canonical_count(value: &str) -> Option<&str> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let normalized = value.trim_start_matches('0');
    Some(if normalized.is_empty() {
        "0"
    } else {
        normalized
    })
}

fn top_window(
    state: &ApiState,
    limit: u16,
    returned: usize,
    total: &str,
) -> Result<AuditTopWindow, ApiFailure> {
    let returned_count = u16::try_from(returned).map_err(|_| invalid_response(state))?;
    if returned_count > limit {
        return Err(invalid_response(state));
    }
    let total = canonical_count(total).ok_or_else(|| invalid_response(state))?;
    let returned_text = returned_count.to_string();
    let truncated = (total.len(), total) > (returned_text.len(), returned_text.as_str());
    // A separate later coverage read may observe retention/deletion. Preserve
    // that non-atomic limitation rather than inventing a new total from Top.
    Ok(AuditTopWindow {
        requested_limit: limit,
        returned_count,
        full_window_count: total.to_owned(),
        truncated,
    })
}

fn invalid_response(state: &ApiState) -> ApiFailure {
    ApiFailure::new(
        state.next_request_id(),
        StatusCode::BAD_GATEWAY,
        "connector_response_invalid",
        "audit_workload",
        "audit_projection_invalid",
        "retry_same",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> ApiState {
        ApiState::new(ApiConfig::default()).unwrap()
    }

    fn family_identity() -> AuditSessionIdentity {
        AuditSessionIdentity {
            connection_id: "session-a".into(),
            detected_version: "3.5.14".into(),
            cluster_identity: doctor_connector::AuditClusterIdentity::SessionScoped,
            audit_timestamp_time_zone: None,
        }
    }

    fn family_source_row(
        error_count: Option<&str>,
        execution_count: Option<&str>,
    ) -> AuditFamilySourceRow {
        AuditFamilySourceRow {
            catalog: Some("default_catalog".into()),
            database: Some("demo".into()),
            native_digest: Some("a0b1".into()),
            canonical_execution_count: execution_count.map(str::to_owned),
            error_count: error_count.map(str::to_owned),
            query_time_valid_count: Some("1".into()),
            cumulative_query_time: Some("0".into()),
            scan_bytes_valid_count: Some("1".into()),
            cumulative_scan_bytes: Some("0".into()),
            cpu_cost_valid_count: Some("1".into()),
            cumulative_cpu_cost: Some("0".into()),
            memory_cost_valid_count: Some("1".into()),
            max_memory_cost: Some("0".into()),
            max_query_time: Some("0".into()),
            error_proportion_population_max: None,
            statement_sample: doctor_connector::AuditSourceStatementSample::NotRead,
        }
    }

    #[test]
    fn audit_window_allows_thirty_days_and_early_rejects_one_second_more() {
        let state = state();
        let mut scope = AuditScope {
            window: doctor_contracts::AuditWindow {
                start_seconds: 1,
                end_seconds: 1 + doctor_contracts::MAX_AUDIT_WINDOW_SECONDS as u32,
            },
            state: None,
            user: None,
            native_digest: None,
            catalog: None,
            database: None,
            resource_group: None,
            statement_filter: doctor_contracts::AuditStatementFilter::All,
        };
        assert!(validate_audit_scope_window(&state, &scope).is_ok());
        scope.window.end_seconds += 1;
        let failure = validate_audit_scope_window(&state, &scope).unwrap_err();
        assert_eq!(failure.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(failure.code(), "audit_window_too_wide");
    }

    #[test]
    fn audit_top_compares_exact_decimal_counts_without_floating_point_or_limit_guessing() {
        let state = state();
        assert!(
            top_window(&state, 50, 2, "9007199254740993")
                .unwrap()
                .truncated
        );
        assert!(!top_window(&state, 50, 50, "00050").unwrap().truncated);
        assert!(top_window(&state, 50, 50, "51").unwrap().truncated);
        assert!(!top_window(&state, 50, 0, "0").unwrap().truncated);
        assert!(top_window(&state, 50, 51, "51").is_err());
        for invalid in ["", "-1", "+1", " 1", "1.0", "1e4"] {
            assert!(top_window(&state, 50, 0, invalid).is_err());
            assert!(optional_count(&state, Some(invalid.into())).is_err());
        }
        assert_eq!(optional_count(&state, None).unwrap(), None);
        assert_eq!(
            optional_count(&state, Some("000".into())).unwrap(),
            Some("0".into())
        );
    }

    #[test]
    fn audit_error_proportion_keeps_exact_fraction_states_without_floating_point() {
        assert_eq!(
            error_proportion(Some("000"), Some("9007199254740993")),
            AuditErrorProportion::Exact {
                numerator: "0".into(),
                denominator: "9007199254740993".into(),
            }
        );
        assert_eq!(
            error_proportion(Some("1"), Some("3")),
            AuditErrorProportion::Exact {
                numerator: "1".into(),
                denominator: "3".into(),
            }
        );
        assert_eq!(
            error_proportion(Some("0"), Some("0")),
            AuditErrorProportion::NotApplicable {
                numerator: "0".into(),
                denominator: "0".into(),
            }
        );
        assert_eq!(
            error_proportion(None, Some("3")),
            AuditErrorProportion::Unavailable {
                reason: AuditErrorProportionUnavailableReason::ErrorCountMissing,
            }
        );
        assert_eq!(
            error_proportion(Some("1"), None),
            AuditErrorProportion::Unavailable {
                reason: AuditErrorProportionUnavailableReason::ExecutionCountMissing,
            }
        );
        assert_eq!(
            error_proportion(Some("invalid"), Some("3")),
            AuditErrorProportion::Invalid {
                reason: AuditErrorProportionInvalidReason::ErrorCountInvalid,
            }
        );
        assert_eq!(
            error_proportion(Some("1"), Some("invalid")),
            AuditErrorProportion::Invalid {
                reason: AuditErrorProportionInvalidReason::ExecutionCountInvalid,
            }
        );
        assert_eq!(
            error_proportion(Some("4"), Some("3")),
            AuditErrorProportion::Invalid {
                reason: AuditErrorProportionInvalidReason::NumeratorExceedsDenominator,
            }
        );
    }

    #[test]
    fn family_api_projection_always_carries_the_typed_error_proportion() {
        let state = state();
        let identity = family_identity();
        let mut source = family_source_row(Some("1"), Some("3"));
        source.error_proportion_population_max = Some("3000000000000".into());
        let normal = family_row(&state, &identity, source).unwrap();
        assert_eq!(
            normal.error_proportion,
            AuditErrorProportion::Exact {
                numerator: "1".into(),
                denominator: "3".into(),
            }
        );

        let missing = family_row(&state, &identity, family_source_row(None, Some("3"))).unwrap();
        assert_eq!(missing.error_count, None);
        assert_eq!(
            missing.error_proportion,
            AuditErrorProportion::Unavailable {
                reason: AuditErrorProportionUnavailableReason::ErrorCountMissing,
            }
        );

        let invalid =
            family_row(&state, &identity, family_source_row(Some("x"), Some("3"))).unwrap();
        assert_eq!(invalid.error_count, None);
        assert_eq!(
            invalid.error_proportion,
            AuditErrorProportion::Invalid {
                reason: AuditErrorProportionInvalidReason::ErrorCountInvalid,
            }
        );
    }

    #[test]
    fn audit_missing_timestamp_is_not_fabricated_and_family_identity_is_session_scoped() {
        let state = state();
        let identity = AuditSessionIdentity {
            connection_id: "session-a".into(),
            detected_version: "3.5.14".into(),
            cluster_identity: doctor_connector::AuditClusterIdentity::SessionScoped,
            audit_timestamp_time_zone: None,
        };
        let row = AuditOccurrenceSourceRow {
            query_id: Some("00000000-0000-0000-0000-000000003514".into()),
            event_time: None,
            event_epoch_seconds: Some("1788048000".into()),
            state: Some("EOF".into()),
            metrics: vec![],
            error_code: AuditErrorCodeSource::SqlNull,
            catalog: Some("default_catalog".into()),
            database: Some("demo".into()),
            resource_group: None,
            native_digest: Some("a0b1".into()),
            statement_sample: doctor_connector::AuditSourceStatementSample::NotRead,
        };
        let first = occurrence_row(&state, &identity, 1, 0, row.clone()).unwrap();
        assert_eq!(first.error_code, AuditErrorCodeFact::SqlNull);
        assert_eq!(
            first.query.timestamp_state,
            ConnectedTimestampState::Unavailable
        );
        assert!(first.query.observed_at_utc.is_none());
        let mut other = identity.clone();
        other.connection_id = "session-b".into();
        let second = occurrence_row(&state, &other, 1, 0, row.clone()).unwrap();
        assert_ne!(first.family, second.family);
        let mut missing = row.clone();
        missing.native_digest = None;
        let missing = occurrence_row(&state, &identity, 1, 0, missing).unwrap();
        assert!(matches!(
            missing.family,
            AuditFamilyMembership::Ungrouped { .. }
        ));
        let mut invalid = row;
        invalid.query_id = None;
        assert!(occurrence_row(&state, &identity, 1, 0, invalid).is_err());
    }

    #[test]
    fn occurrence_projection_keeps_optional_fact_unavailable_distinct_from_empty_text() {
        let state = state();
        let identity = AuditSessionIdentity {
            connection_id: "session-a".into(),
            detected_version: "3.5.14".into(),
            cluster_identity: doctor_connector::AuditClusterIdentity::SessionScoped,
            audit_timestamp_time_zone: None,
        };
        let source = |error_code| AuditOccurrenceSourceRow {
            query_id: Some("00000000-0000-0000-0000-000000003514".into()),
            event_time: None,
            event_epoch_seconds: None,
            state: Some("ERR".into()),
            metrics: vec![(
                AuditMetricId::PendingTime,
                AuditMetricInput::Text { value: "0".into() },
            )],
            error_code,
            catalog: None,
            database: None,
            resource_group: None,
            native_digest: None,
            statement_sample: doctor_connector::AuditSourceStatementSample::NotRead,
        };
        let unavailable = occurrence_row(
            &state,
            &identity,
            1,
            0,
            source(AuditErrorCodeSource::Unavailable {
                reason: doctor_contracts::AuditOptionalFactUnavailableReason::SourceColumnAbsent,
            }),
        )
        .unwrap();
        assert_eq!(
            unavailable.error_code,
            AuditErrorCodeFact::Unavailable {
                reason: doctor_contracts::AuditOptionalFactUnavailableReason::SourceColumnAbsent,
            }
        );
        assert_eq!(unavailable.metrics[0].metric_id, AuditMetricId::PendingTime);
        assert_eq!(unavailable.metrics[0].state, AuditMetricState::Exact);
        let empty = occurrence_row(
            &state,
            &identity,
            1,
            0,
            source(AuditErrorCodeSource::Text {
                value: String::new(),
            }),
        )
        .unwrap();
        assert_eq!(
            empty.error_code,
            AuditErrorCodeFact::Text {
                value: String::new(),
            }
        );
    }

    #[test]
    fn occurrence_statement_sample_keeps_every_source_state_distinct() {
        use doctor_connector::AuditSourceStatementSample as Source;
        let text = |value: &str, truncated| {
            Source::Read(AuditStatementSampleSource::Text {
                value: value.into(),
                truncated,
            })
        };
        let unavailable = |reason| AuditStatementSample::Unavailable { reason };
        for (source, expected) in [
            (
                Source::NotRead,
                unavailable(AuditStatementSampleUnavailableReason::NotRead),
            ),
            (
                Source::SourceColumnAbsent,
                unavailable(AuditStatementSampleUnavailableReason::SourceColumnAbsent),
            ),
            (
                Source::ConflictingSourceRows,
                unavailable(AuditStatementSampleUnavailableReason::ConflictingSourceRows),
            ),
            (
                Source::Read(AuditStatementSampleSource::SqlNull),
                AuditStatementSample::SqlNull,
            ),
            // An empty audited statement stays text, never NULL or missing.
            (
                text("", false),
                AuditStatementSample::Text {
                    value: String::new(),
                    truncated: false,
                },
            ),
            (
                text("SELECT 1", true),
                AuditStatementSample::Text {
                    value: "SELECT 1".into(),
                    truncated: true,
                },
            ),
        ] {
            assert_eq!(statement_sample(source), expected);
        }
    }
}
