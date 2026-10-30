//! Load workspace (tasks 51, 52): Routine Load jobs, recent load records,
//! their overview and one load's error log, read live through the connected
//! session. Nothing is stored. Error log rows are returned only for the one
//! job asked for.
use doctor_contracts::{
    LoadErrorLogRequest, LoadErrorLogResponse, LoadErrorLogState, LoadOverviewRequest,
    LoadReadRequest, LoadRecordsRequest,
};
use doctor_core::parse_load_error_log;

use super::*;
use crate::audit::no_store;

fn begin_load_read(state: &ApiState) -> Result<WorkLease, ApiFailure> {
    state
        .budgets
        .begin(WorkKind::Analysis)
        .map_err(|violation| violation.failure(state.next_request_id()))
}

fn invalid(state: &ApiState, code: &'static str) -> ApiFailure {
    ApiFailure::new(
        state.next_request_id(),
        StatusCode::BAD_REQUEST,
        code,
        "loads",
        code,
        "change_input",
    )
}

pub(super) async fn routine_jobs(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: LoadReadRequest = connector_json_request(&state, request).await?;
    if !request.validate() {
        return Err(invalid(&state, "invalid_load_request"));
    }
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    let lease = begin_load_read(&state)?;
    audit_connector_call(
        &state,
        lease,
        connection_id.clone(),
        move |sessions, control, state| {
            let response = sessions
                .routine_load_jobs_controlled(&connection_id, Some(control))
                .map_err(|error| connector_failure(state, error))?;
            Ok(no_store(response))
        },
    )
    .await
}

pub(super) async fn records(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: LoadRecordsRequest = connector_json_request(&state, request).await?;
    if !request.validate() {
        return Err(invalid(&state, "invalid_load_request"));
    }
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    let lease = begin_load_read(&state)?;
    audit_connector_call(
        &state,
        lease,
        connection_id.clone(),
        move |sessions, control, state| {
            let response = sessions
                .load_records_controlled(&connection_id, &request, Some(control))
                .map_err(|error| connector_failure(state, error))?;
            Ok(no_store(response))
        },
    )
    .await
}

pub(super) async fn overview(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: LoadOverviewRequest = connector_json_request(&state, request).await?;
    if !request.validate() {
        return Err(invalid(&state, "invalid_load_request"));
    }
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    let lease = begin_load_read(&state)?;
    audit_connector_call(
        &state,
        lease,
        connection_id.clone(),
        move |sessions, control, state| {
            let response = sessions
                .load_overview_controlled(&connection_id, &request, Some(control))
                .map_err(|error| connector_failure(state, error))?;
            Ok(no_store(response))
        },
    )
    .await
}

pub(super) async fn error_log(
    State(state): State<ApiState>,
    AxumPath(connection_id): AxumPath<String>,
    request: Request,
) -> Result<Response, ApiFailure> {
    let request: LoadErrorLogRequest = connector_json_request(&state, request).await?;
    if !request.validate() {
        return Err(invalid(&state, "invalid_load_error_log_request"));
    }
    require_connection_id(&state, &connection_id, &request.connection_id)?;
    let job_id: u64 = request
        .job_id
        .parse()
        .map_err(|_| invalid(&state, "invalid_load_error_log_request"))?;
    let lease = begin_load_read(&state)?;
    audit_connector_call(
        &state,
        lease,
        connection_id.clone(),
        move |sessions, control, state| {
            let text = sessions
                .load_tracking_log_controlled(&connection_id, job_id, Some(control))
                .map_err(|error| connector_failure(state, error))?;
            let parsed = parse_load_error_log(text.as_deref());
            let mut limitations = vec![
                "sample_of_about_50_rows_per_file".to_owned(),
                "first_error_per_row_only".to_owned(),
            ];
            if parsed.state == LoadErrorLogState::Purged {
                limitations.push("be_keeps_error_logs_48_hours".to_owned());
            }
            if parsed.state == LoadErrorLogState::Unavailable {
                limitations.push("be_unreachable_try_again".to_owned());
            }
            Ok(no_store(LoadErrorLogResponse {
                job_id: job_id.to_string(),
                state: parsed.state,
                entries: parsed.entries,
                categories: parsed.categories,
                truncated: parsed.truncated,
                limitations,
            }))
        },
    )
    .await
}
