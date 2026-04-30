use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use ingot_domain::finding::{Finding, FindingTriageState};
use ingot_domain::ids::FindingId;
use ingot_usecases::finding_commands as finding_uc;
use tracing::warn;

use crate::error::ApiError;

use super::app::AppState;
use super::support::{errors::repo_to_finding, path::ApiPath};
use super::types::*;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/findings/{finding_id}", get(get_finding))
        .route(
            "/api/findings/{finding_id}/triage",
            post(triage_item_finding),
        )
        .route(
            "/api/findings/{finding_id}/promote",
            post(promote_item_from_finding),
        )
        .route(
            "/api/findings/{finding_id}/dismiss",
            post(dismiss_item_finding),
        )
        .route(
            "/api/projects/{project_id}/findings/batch-promote",
            post(batch_promote_findings_handler),
        )
}

pub(super) async fn get_finding(
    State(state): State<AppState>,
    ApiPath(FindingPathParams { finding_id }): ApiPath<FindingPathParams>,
) -> Result<Json<Finding>, ApiError> {
    let finding = state
        .db
        .get_finding(finding_id)
        .await
        .map_err(repo_to_finding)?;
    Ok(Json(finding))
}

pub(super) async fn triage_item_finding(
    State(state): State<AppState>,
    ApiPath(FindingPathParams { finding_id }): ApiPath<FindingPathParams>,
    request: TriageFindingRequest,
) -> Result<Json<Finding>, ApiError> {
    let applied = finding_uc::apply_finding_triage(
        &state.db,
        &state.infra(),
        &state.project_locks,
        triage_command(finding_id, request),
    )
    .await?;
    log_auto_dispatch_error(&applied, "finding triage");
    Ok(Json(applied.finding))
}

pub(super) async fn dismiss_item_finding(
    State(state): State<AppState>,
    ApiPath(FindingPathParams { finding_id }): ApiPath<FindingPathParams>,
    Json(request): Json<DismissFindingRequest>,
) -> Result<Json<Finding>, ApiError> {
    let applied = finding_uc::apply_finding_triage(
        &state.db,
        &state.infra(),
        &state.project_locks,
        finding_uc::TriageFindingCommand {
            finding_id,
            triage_state: FindingTriageState::DismissedInvalid,
            triage_note: Some(request.dismissal_reason),
            linked_item_id: None,
            target_ref: None,
            approval_policy: None,
        },
    )
    .await?;
    log_auto_dispatch_error(&applied, "finding dismiss");
    Ok(Json(applied.finding))
}

pub(super) async fn promote_item_from_finding(
    State(state): State<AppState>,
    ApiPath(FindingPathParams { finding_id }): ApiPath<FindingPathParams>,
    maybe_request: Option<Json<PromoteFindingRequest>>,
) -> Result<Json<PromoteFindingResponse>, ApiError> {
    let PromoteFindingRequest {
        target_ref,
        approval_policy,
        dispatch_immediately,
    } = maybe_request
        .map(|Json(request)| request)
        .unwrap_or_default();
    let dispatch_immediately = dispatch_immediately.unwrap_or(false);

    let output = finding_uc::promote_finding(
        &state.db,
        &state.infra(),
        &state.project_locks,
        finding_uc::PromoteFindingCommand {
            finding_id,
            target_ref,
            approval_policy,
            dispatch_immediately,
        },
    )
    .await?;
    let (launch_status, job, launch_error) = match output.launch {
        finding_uc::PromoteFindingLaunch::NotRequested => {
            (PromoteFindingLaunchStatus::NotRequested, None, None)
        }
        finding_uc::PromoteFindingLaunch::Dispatched(job) => {
            (PromoteFindingLaunchStatus::Dispatched, Some(*job), None)
        }
        finding_uc::PromoteFindingLaunch::NoDispatchableStep => (
            PromoteFindingLaunchStatus::DispatchFailed,
            None,
            Some("No dispatchable step was available on the promoted item".into()),
        ),
        finding_uc::PromoteFindingLaunch::DispatchFailed(error) => (
            PromoteFindingLaunchStatus::DispatchFailed,
            None,
            Some(format!("{error:?}")),
        ),
    };

    Ok(Json(PromoteFindingResponse {
        item: output.item,
        current_revision: output.current_revision,
        finding: output.finding,
        launch_status,
        job,
        launch_error,
    }))
}

fn triage_command(
    finding_id: FindingId,
    request: TriageFindingRequest,
) -> finding_uc::TriageFindingCommand {
    finding_uc::TriageFindingCommand {
        finding_id,
        triage_state: request.triage_state,
        triage_note: request.triage_note,
        linked_item_id: request.linked_item_id,
        target_ref: request.target_ref,
        approval_policy: request.approval_policy,
    }
}

fn log_auto_dispatch_error(applied: &finding_uc::AppliedFindingTriage, context: &'static str) {
    if let Some(Err(error)) = &applied.auto_dispatch_result {
        warn!(?error, finding_id = %applied.finding.id, context, "projected review auto-dispatch failed after finding triage");
    }
}

pub(super) async fn batch_promote_findings_handler(
    State(state): State<AppState>,
    ApiPath(ProjectPathParams { project_id }): ApiPath<ProjectPathParams>,
    Json(request): Json<BatchPromoteFindingsRequest>,
) -> Result<Json<BatchPromoteFindingsResponse>, ApiError> {
    let output = finding_uc::batch_promote_findings_command(
        &state.db,
        &state.project_locks,
        finding_uc::BatchPromoteFindingsCommand {
            project_id,
            finding_ids: request.finding_ids,
        },
    )
    .await?;

    let response = BatchPromoteFindingsResponse {
        promoted: output
            .promoted
            .into_iter()
            .map(|r| PromotedFindingResult {
                finding_id: r.finding_id,
                item: r.linked_item,
                current_revision: r.linked_revision,
            })
            .collect(),
        skipped: output
            .skipped
            .into_iter()
            .map(|s| SkippedFindingResult {
                finding_id: s.finding_id,
                reason: s.reason,
            })
            .collect(),
    };

    Ok(Json(response))
}
