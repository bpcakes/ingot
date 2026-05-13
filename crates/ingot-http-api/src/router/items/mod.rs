use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use ingot_domain::activity::ActivityEventType;
use ingot_domain::commit_oid::CommitOid;
use ingot_domain::finding::Finding;
use ingot_domain::ids::{ItemId, ProjectId};
use ingot_domain::item::DoneReason;
use ingot_domain::job::Job;
use ingot_domain::revision::ItemRevision;
use ingot_usecases::UseCaseError;
use ingot_usecases::item_commands as item_uc;
use ingot_workflow::Evaluator;
use tracing::warn;

use crate::error::ApiError;
pub(super) use revisions::build_superseding_revision;

use super::app::AppState;
use super::item_projection::{
    evaluate_item_snapshot, load_item_detail, load_item_runtime_snapshot,
};
use super::support::{
    config::load_effective_config,
    errors::{repo_to_internal, repo_to_item, repo_to_project},
    path::ApiPath,
};
use super::types::*;

mod revisions;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/projects/{project_id}/items",
            get(list_items).post(create_item),
        )
        .route(
            "/api/projects/{project_id}/items/{item_id}",
            get(get_item).patch(update_item),
        )
        .route(
            "/api/projects/{project_id}/items/{item_id}/revise",
            post(revise_item),
        )
        .route(
            "/api/projects/{project_id}/items/{item_id}/defer",
            post(defer_item),
        )
        .route(
            "/api/projects/{project_id}/items/{item_id}/resume",
            post(resume_item),
        )
        .route(
            "/api/projects/{project_id}/items/{item_id}/dismiss",
            post(dismiss_item),
        )
        .route(
            "/api/projects/{project_id}/items/{item_id}/invalidate",
            post(invalidate_item),
        )
        .route(
            "/api/projects/{project_id}/items/{item_id}/reopen",
            post(reopen_item),
        )
        .route(
            "/api/projects/{project_id}/items/{item_id}/findings",
            get(list_item_findings),
        )
}

pub(super) async fn create_item(
    State(state): State<AppState>,
    ApiPath(ProjectPathParams { project_id }): ApiPath<ProjectPathParams>,
    Json(request): Json<CreateItemRequest>,
) -> Result<(StatusCode, Json<ItemDetailResponse>), ApiError> {
    let project = state
        .db()
        .get_project(project_id)
        .await
        .map_err(repo_to_project)?;
    let config = load_effective_config(Some(&project))?;
    let output = item_uc::create_item(
        state.db(),
        &state.infra(),
        state.project_locks(),
        item_uc::CreateItemCommand {
            project_id,
            title: request.title,
            description: request.description,
            acceptance_criteria: request.acceptance_criteria,
            classification: request.classification,
            priority: request.priority,
            labels: request.labels,
            operator_notes: request.operator_notes,
            target_ref: request.target_ref,
            approval_policy: request.approval_policy,
            seed_commit_oid: request.seed_commit_oid,
            seed_target_commit_oid: request.seed_target_commit_oid,
            default_approval_policy: config.defaults.approval_policy,
            candidate_rework_budget: config.defaults.candidate_rework_budget,
            integration_rework_budget: config.defaults.integration_rework_budget,
        },
    )
    .await?;

    let detail = load_item_detail(&state, project_id, output.item_id).await?;
    Ok((StatusCode::CREATED, Json(detail)))
}

pub(super) async fn list_items(
    State(state): State<AppState>,
    ApiPath(ProjectPathParams { project_id }): ApiPath<ProjectPathParams>,
) -> Result<Json<Vec<ItemSummaryResponse>>, ApiError> {
    let project = state
        .db()
        .get_project(project_id)
        .await
        .map_err(repo_to_project)?;
    let items = state
        .db()
        .list_items_by_project(project_id)
        .await
        .map_err(repo_to_internal)?;
    let evaluator = Evaluator::new();
    let mut summaries = Vec::with_capacity(items.len());

    for item in items {
        let snapshot = load_item_runtime_snapshot(&state, project.id, &item).await?;
        let (evaluation, finalization, queue) =
            evaluate_item_snapshot(&state, &project, &item, &snapshot, &evaluator).await?;

        let title = snapshot.current_revision.title.clone();
        summaries.push(ItemSummaryResponse {
            item,
            title,
            evaluation,
            finalization,
            queue,
        });
    }

    Ok(Json(summaries))
}

pub(super) async fn update_item(
    State(state): State<AppState>,
    ApiPath(ProjectItemPathParams {
        project_id,
        item_id,
    }): ApiPath<ProjectItemPathParams>,
    Json(request): Json<UpdateItemRequest>,
) -> Result<Json<ItemDetailResponse>, ApiError> {
    let output = item_uc::update_item(
        state.db(),
        state.project_locks(),
        item_uc::UpdateItemCommand {
            project_id,
            item_id,
            classification: request.classification,
            priority: request.priority,
            labels: request.labels,
            operator_notes: request.operator_notes,
        },
    )
    .await?;
    let detail = load_item_detail(&state, project_id, output.item_id).await?;
    Ok(Json(detail))
}

pub(super) async fn get_item(
    State(state): State<AppState>,
    ApiPath(ProjectItemPathParams {
        project_id,
        item_id,
    }): ApiPath<ProjectItemPathParams>,
) -> Result<Json<ItemDetailResponse>, ApiError> {
    state
        .db()
        .get_project(project_id)
        .await
        .map_err(repo_to_project)?;
    let response = load_item_detail(&state, project_id, item_id).await?;
    Ok(Json(response))
}

pub(super) async fn revise_item(
    State(state): State<AppState>,
    ApiPath(ProjectItemPathParams {
        project_id,
        item_id,
    }): ApiPath<ProjectItemPathParams>,
    maybe_request: Option<Json<ReviseItemRequest>>,
) -> Result<Json<ItemDetailResponse>, ApiError> {
    let request: ReviseItemRequest = maybe_request
        .map(|Json(request)| request)
        .unwrap_or_default();
    let output = item_uc::revise_item(
        state.db(),
        &state.infra(),
        state.project_locks(),
        project_id,
        item_id,
        revise_command(request),
    )
    .await?;
    let detail = load_item_detail(&state, project_id, output.item_id).await?;
    Ok(Json(detail))
}

pub(super) async fn defer_item(
    State(state): State<AppState>,
    ApiPath(ProjectItemPathParams {
        project_id,
        item_id,
    }): ApiPath<ProjectItemPathParams>,
) -> Result<Json<ItemDetailResponse>, ApiError> {
    let output = item_uc::defer_item(
        state.db(),
        &state.infra(),
        state.project_locks(),
        project_id,
        item_id,
    )
    .await?;
    let detail = load_item_detail(&state, project_id, output.item_id).await?;
    Ok(Json(detail))
}

pub(super) async fn resume_item(
    State(state): State<AppState>,
    ApiPath(ProjectItemPathParams {
        project_id,
        item_id,
    }): ApiPath<ProjectItemPathParams>,
) -> Result<Json<ItemDetailResponse>, ApiError> {
    let (output, dispatch_result) = item_uc::resume_item(
        state.db(),
        &state.infra(),
        state.project_locks(),
        project_id,
        item_id,
    )
    .await?;
    if let Err(error) = dispatch_result {
        warn!(
            ?error,
            project_id = %project_id,
            item_id = %output.item_id,
            "projected review auto-dispatch failed after resume"
        );
    }
    let detail = load_item_detail(&state, project_id, output.item_id).await?;
    Ok(Json(detail))
}

pub(super) async fn dismiss_item(
    State(state): State<AppState>,
    ApiPath(ProjectItemPathParams {
        project_id,
        item_id,
    }): ApiPath<ProjectItemPathParams>,
) -> Result<Json<ItemDetailResponse>, ApiError> {
    finish_item_manually(
        state,
        project_id,
        item_id,
        DoneReason::Dismissed,
        ActivityEventType::ItemDismissed,
    )
    .await
}

pub(super) async fn invalidate_item(
    State(state): State<AppState>,
    ApiPath(ProjectItemPathParams {
        project_id,
        item_id,
    }): ApiPath<ProjectItemPathParams>,
) -> Result<Json<ItemDetailResponse>, ApiError> {
    finish_item_manually(
        state,
        project_id,
        item_id,
        DoneReason::Invalidated,
        ActivityEventType::ItemInvalidated,
    )
    .await
}

pub(super) async fn reopen_item(
    State(state): State<AppState>,
    ApiPath(ProjectItemPathParams {
        project_id,
        item_id,
    }): ApiPath<ProjectItemPathParams>,
    maybe_request: Option<Json<ReviseItemRequest>>,
) -> Result<Json<ItemDetailResponse>, ApiError> {
    let request: ReviseItemRequest = maybe_request
        .map(|Json(request)| request)
        .unwrap_or_default();
    let output = item_uc::reopen_item(
        state.db(),
        &state.infra(),
        state.project_locks(),
        project_id,
        item_id,
        revise_command(request),
    )
    .await?;
    let detail = load_item_detail(&state, project_id, output.item_id).await?;
    Ok(Json(detail))
}

pub(super) async fn list_item_findings(
    State(state): State<AppState>,
    ApiPath(ProjectItemPathParams {
        project_id,
        item_id,
    }): ApiPath<ProjectItemPathParams>,
) -> Result<Json<Vec<Finding>>, ApiError> {
    state
        .db()
        .get_project(project_id)
        .await
        .map_err(repo_to_project)?;
    let item = state.db().get_item(item_id).await.map_err(repo_to_item)?;
    if item.project_id != project_id {
        return Err(UseCaseError::ItemNotFound.into());
    }

    let findings = state
        .db()
        .list_findings_by_item(item_id)
        .await
        .map_err(repo_to_internal)?;

    Ok(Json(findings))
}

pub(super) async fn finish_item_manually(
    state: AppState,
    project_id: ProjectId,
    item_id: ItemId,
    done_reason: DoneReason,
    event_type: ActivityEventType,
) -> Result<Json<ItemDetailResponse>, ApiError> {
    let output = item_uc::finish_item_manually(
        state.db(),
        &state.infra(),
        state.project_locks(),
        project_id,
        item_id,
        done_reason,
        event_type,
    )
    .await?;
    let detail = load_item_detail(&state, project_id, output.item_id).await?;
    Ok(Json(detail))
}

fn revise_command(request: ReviseItemRequest) -> item_uc::ReviseItemCommand {
    item_uc::ReviseItemCommand {
        title: request.title,
        description: request.description,
        acceptance_criteria: request.acceptance_criteria,
        target_ref: request.target_ref,
        approval_policy: request.approval_policy,
        seed_commit_oid: request.seed_commit_oid,
        seed_target_commit_oid: request.seed_target_commit_oid,
    }
}

pub(super) async fn current_authoring_head_for_revision_with_workspace(
    state: &AppState,
    revision: &ItemRevision,
    jobs: &[Job],
) -> Result<Option<CommitOid>, ApiError> {
    let workspace = state
        .db()
        .find_authoring_workspace_for_revision(revision.id)
        .await
        .map_err(repo_to_internal)?;
    Ok(
        ingot_usecases::dispatch::current_authoring_head_for_revision_with_workspace(
            revision,
            jobs,
            workspace.as_ref(),
        ),
    )
}

pub(super) async fn effective_authoring_base_commit_oid(
    state: &AppState,
    revision: &ItemRevision,
) -> Result<Option<CommitOid>, ApiError> {
    let workspace = state
        .db()
        .find_authoring_workspace_for_revision(revision.id)
        .await
        .map_err(repo_to_internal)?;
    Ok(ingot_usecases::dispatch::effective_authoring_base_commit_oid(revision, workspace.as_ref()))
}
