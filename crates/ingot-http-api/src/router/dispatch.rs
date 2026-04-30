use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use ingot_domain::job::Job;
use ingot_domain::ports::ProjectMutationLockPort;
use ingot_usecases::UseCaseError;
use ingot_usecases::dispatch::{DispatchActivityContext, prepare_and_persist_dispatched_job};
use ingot_usecases::job::{DispatchJobCommand, dispatch_job, retry_job};

use crate::error::ApiError;

use super::app::AppState;
use super::item_projection::{ItemRuntimeSnapshot, load_item_runtime_snapshot};
use super::support::{
    errors::{repo_to_item, repo_to_project},
    path::ApiPath,
};
use super::types::*;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/projects/{project_id}/items/{item_id}/jobs",
            post(dispatch_item_job),
        )
        .route(
            "/api/projects/{project_id}/items/{item_id}/jobs/{job_id}/retry",
            post(retry_item_job),
        )
}

pub(super) async fn dispatch_item_job(
    State(state): State<AppState>,
    ApiPath(ProjectItemPathParams {
        project_id,
        item_id,
    }): ApiPath<ProjectItemPathParams>,
    maybe_request: Option<Json<DispatchJobRequest>>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    let project = state
        .db
        .get_project(project_id)
        .await
        .map_err(repo_to_project)?;
    let _guard = state
        .project_locks
        .acquire_project_mutation(project_id)
        .await;

    let item = state.db.get_item(item_id).await.map_err(repo_to_item)?;
    if item.project_id != project_id {
        return Err(UseCaseError::ItemNotFound.into());
    }

    let ItemRuntimeSnapshot {
        current_revision,
        jobs,
        findings,
        convergences,
    } = load_item_runtime_snapshot(&state, project.id, &item).await?;
    let command = DispatchJobCommand {
        step_id: maybe_request.and_then(|Json(request)| request.step_id),
    };
    let job = dispatch_job(
        &item,
        &current_revision,
        &jobs,
        &findings,
        &convergences,
        command,
    )?;
    let infra = state.infra();
    let prepared = prepare_and_persist_dispatched_job(
        &state.db,
        &state.db,
        &state.db,
        &state.db,
        &infra,
        &project,
        &item,
        &current_revision,
        &jobs,
        job,
        DispatchActivityContext {
            dispatch_origin: Some("operator"),
            supersedes_job_id: None,
            retry_no: None,
        },
    )
    .await?;
    let job = prepared.job;

    Ok((StatusCode::CREATED, Json(job)))
}

pub(super) async fn retry_item_job(
    State(state): State<AppState>,
    ApiPath(ProjectItemJobPathParams {
        project_id,
        item_id,
        job_id,
    }): ApiPath<ProjectItemJobPathParams>,
) -> Result<(StatusCode, Json<Job>), ApiError> {
    let project = state
        .db
        .get_project(project_id)
        .await
        .map_err(repo_to_project)?;
    let _guard = state
        .project_locks
        .acquire_project_mutation(project_id)
        .await;

    let item = state.db.get_item(item_id).await.map_err(repo_to_item)?;
    if item.project_id != project_id {
        return Err(UseCaseError::ItemNotFound.into());
    }
    let ItemRuntimeSnapshot {
        current_revision,
        jobs,
        findings,
        convergences,
    } = load_item_runtime_snapshot(&state, project.id, &item).await?;
    let previous_job = jobs
        .iter()
        .find(|job| job.id == job_id)
        .cloned()
        .ok_or_else(|| ApiError::NotFound {
            code: "job_not_found",
            message: "Job not found".into(),
        })?;

    let job = retry_job(
        &item,
        &current_revision,
        &jobs,
        &findings,
        &convergences,
        &previous_job,
    )?;
    let retry_no = job.retry_no;
    let infra = state.infra();
    let prepared = prepare_and_persist_dispatched_job(
        &state.db,
        &state.db,
        &state.db,
        &state.db,
        &infra,
        &project,
        &item,
        &current_revision,
        &jobs,
        job,
        DispatchActivityContext {
            dispatch_origin: None,
            supersedes_job_id: Some(previous_job.id),
            retry_no: Some(retry_no),
        },
    )
    .await?;
    let job = prepared.job;

    Ok((StatusCode::CREATED, Json(job)))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::Utc;
    use ingot_domain::commit_oid::CommitOid;
    use ingot_domain::git_operation::{
        GitOperation, GitOperationEntityRef, GitOperationStatus, OperationPayload,
    };
    use ingot_domain::git_ref::GitRef;
    use ingot_domain::ids::{GitOperationId, JobId, WorkspaceId};
    use ingot_domain::test_support::WorkspaceBuilder;
    use ingot_domain::workspace::{WorkspaceKind, WorkspaceStatus};
    use ingot_git::commands::resolve_ref_oid;
    use ingot_git::project_repo::{ensure_mirror, project_repo_paths};
    use ingot_test_support::git::{
        git_output as support_git_output, run_git as support_git,
        temp_git_repo as support_temp_git_repo,
    };
    use uuid::Uuid;

    use super::super::test_helpers::{test_app_state, test_project};
    fn temp_git_repo() -> PathBuf {
        support_temp_git_repo("ingot-http-api")
    }

    fn git(path: &std::path::Path, args: &[&str]) {
        support_git(path, args);
    }

    fn git_output(path: &std::path::Path, args: &[&str]) -> String {
        support_git_output(path, args)
    }

    #[tokio::test]
    async fn cleanup_failed_dispatch_side_effects_removes_workspace_and_investigation_ref() {
        let repo = temp_git_repo();
        let head = git_output(&repo, &["rev-parse", "HEAD"]);
        let state = test_app_state().await;
        let project = test_project(repo.clone());
        state
            .db
            .create_project(&project)
            .await
            .expect("create project");

        let paths = project_repo_paths(state.state_root.as_path(), project.id, &repo);
        ensure_mirror(&paths).await.expect("ensure mirror");

        let workspace_id = WorkspaceId::from_uuid(Uuid::now_v7());
        let workspace_ref = format!("refs/ingot/workspaces/{workspace_id}");
        git(
            &paths.mirror_git_dir,
            &["update-ref", &workspace_ref, &head],
        );
        let workspace_path = state
            .state_root
            .join(format!("cleanup-workspace-{}", Uuid::now_v7()));
        git(
            &paths.mirror_git_dir,
            &[
                "worktree",
                "add",
                "--detach",
                workspace_path.to_str().expect("workspace path"),
                &workspace_ref,
            ],
        );
        let workspace = WorkspaceBuilder::new(project.id, WorkspaceKind::Authoring)
            .id(workspace_id)
            .path(workspace_path.display().to_string())
            .workspace_ref(workspace_ref.clone())
            .base_commit_oid(head.clone())
            .head_commit_oid(head.clone())
            .status(WorkspaceStatus::Ready)
            .created_at(Utc::now())
            .build();
        state
            .db
            .create_workspace(&workspace)
            .await
            .expect("create workspace row");

        let investigation_ref = format!(
            "refs/ingot/investigations/{}",
            JobId::from_uuid(Uuid::now_v7())
        );
        git(
            &paths.mirror_git_dir,
            &["update-ref", &investigation_ref, &head],
        );
        state
            .db
            .create_git_operation(&GitOperation {
                id: GitOperationId::new(),
                project_id: project.id,
                entity: GitOperationEntityRef::Job(JobId::from_uuid(Uuid::now_v7())),
                payload: OperationPayload::CreateInvestigationRef {
                    ref_name: GitRef::new(&investigation_ref),
                    new_oid: CommitOid::new(&head),
                    commit_oid: Some(CommitOid::new(&head)),
                },
                status: GitOperationStatus::Applied,
                created_at: Utc::now(),
                completed_at: Some(Utc::now()),
            })
            .await
            .expect("create git operation");

        let infra = super::super::infra_ports::HttpInfraAdapter::new(&state);
        ingot_usecases::dispatch::cleanup_failed_dispatch(
            &state.db,
            &state.db,
            &infra,
            project.id,
            Some(&workspace),
            Some(&GitRef::new(&investigation_ref)),
        )
        .await;

        assert!(!workspace_path.exists(), "workspace path removed");
        assert_eq!(
            resolve_ref_oid(paths.mirror_git_dir.as_path(), &GitRef::new(&workspace_ref))
                .await
                .expect("resolve workspace ref"),
            None
        );
        assert_eq!(
            resolve_ref_oid(
                paths.mirror_git_dir.as_path(),
                &GitRef::new(&investigation_ref)
            )
            .await
            .expect("resolve investigation ref"),
            None
        );
        let workspace_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workspaces WHERE id = ?")
                .bind(workspace.id.to_string())
                .fetch_one(state.db.raw_pool())
                .await
                .expect("workspace count");
        assert_eq!(workspace_count, 0);
        let op_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM git_operations WHERE operation_kind = 'create_investigation_ref' AND ref_name = ?",
        )
        .bind(&investigation_ref)
        .fetch_one(state.db.raw_pool())
        .await
        .expect("operation count");
        assert_eq!(op_count, 0);
    }

    #[tokio::test]
    async fn cleanup_failed_dispatch_side_effects_deletes_db_rows_when_mirror_refresh_fails() {
        let state = test_app_state().await;
        let missing_repo = state
            .state_root
            .join(format!("missing-repo-{}", Uuid::now_v7()));
        let project = test_project(missing_repo);
        state
            .db
            .create_project(&project)
            .await
            .expect("create project");

        let workspace = WorkspaceBuilder::new(project.id, WorkspaceKind::Authoring)
            .id(WorkspaceId::from_uuid(Uuid::now_v7()))
            .path(
                state
                    .state_root
                    .join(format!("orphaned-workspace-{}", Uuid::now_v7()))
                    .display()
                    .to_string(),
            )
            .workspace_ref(format!(
                "refs/ingot/workspaces/{}",
                WorkspaceId::from_uuid(Uuid::now_v7())
            ))
            .base_commit_oid("deadbeef".repeat(5))
            .head_commit_oid("deadbeef".repeat(5))
            .status(WorkspaceStatus::Ready)
            .created_at(Utc::now())
            .build();
        state
            .db
            .create_workspace(&workspace)
            .await
            .expect("create workspace row");

        let investigation_ref = format!(
            "refs/ingot/investigations/{}",
            JobId::from_uuid(Uuid::now_v7())
        );
        state
            .db
            .create_git_operation(&GitOperation {
                id: GitOperationId::new(),
                project_id: project.id,
                entity: GitOperationEntityRef::Job(JobId::from_uuid(Uuid::now_v7())),
                payload: OperationPayload::CreateInvestigationRef {
                    ref_name: GitRef::new(&investigation_ref),
                    new_oid: CommitOid::new("deadbeef".repeat(5)),
                    commit_oid: Some(CommitOid::new("deadbeef".repeat(5))),
                },
                status: GitOperationStatus::Applied,
                created_at: Utc::now(),
                completed_at: Some(Utc::now()),
            })
            .await
            .expect("create git operation");

        let infra = super::super::infra_ports::HttpInfraAdapter::new(&state);
        ingot_usecases::dispatch::cleanup_failed_dispatch(
            &state.db,
            &state.db,
            &infra,
            project.id,
            Some(&workspace),
            Some(&GitRef::new(&investigation_ref)),
        )
        .await;

        let workspace_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workspaces WHERE id = ?")
                .bind(workspace.id.to_string())
                .fetch_one(state.db.raw_pool())
                .await
                .expect("workspace count");
        assert_eq!(workspace_count, 0);
        let op_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM git_operations WHERE operation_kind = 'create_investigation_ref' AND ref_name = ?",
        )
        .bind(&investigation_ref)
        .fetch_one(state.db.raw_pool())
        .await
        .expect("operation count");
        assert_eq!(op_count, 0);
    }
}
