use std::future::Future;
use std::path::Path;

use chrono::Utc;
use ingot_domain::commit_oid::CommitOid;
use ingot_domain::git_operation::{
    GitOperation, GitOperationEntityRef, GitOperationStatus, OperationPayload,
};
use ingot_domain::git_ref::GitRef;
use ingot_domain::ids::{GitOperationId, ProjectId};
use ingot_domain::ports::WorkspaceRepository;
use ingot_domain::workspace::{Workspace, WorkspaceKind, WorkspaceStatus};

use crate::UseCaseError;
use crate::git_operation_journal::{create_planned, mark_applied};
use crate::store::WorkspaceCommandStore;

pub async fn abandon_workspace<S: WorkspaceRepository>(
    store: &S,
    workspace: &Workspace,
) -> Result<Workspace, UseCaseError> {
    if workspace.state.status() == WorkspaceStatus::Abandoned {
        return Ok(workspace.clone());
    }
    let mut updated = workspace.clone();
    updated.mark_abandoned(Utc::now());
    <S as WorkspaceRepository>::update(store, &updated).await?;
    Ok(updated)
}

pub async fn plan_workspace_removal<S: WorkspaceRepository>(
    store: &S,
    workspace: &Workspace,
) -> Result<Workspace, UseCaseError> {
    let mut updated = workspace.clone();
    updated.mark_removing(Utc::now());
    <S as WorkspaceRepository>::update(store, &updated).await?;
    Ok(updated)
}

pub async fn finalize_workspace_removal<S: WorkspaceRepository>(
    store: &S,
    workspace: &Workspace,
) -> Result<Workspace, UseCaseError> {
    let mut updated = workspace.clone();
    updated.mark_abandoned(Utc::now());
    <S as WorkspaceRepository>::update(store, &updated).await?;
    Ok(updated)
}

pub trait WorkspaceInfraPort: Send + Sync {
    fn reset_worktree(
        &self,
        project_id: ProjectId,
        workspace_path: &Path,
        workspace_ref: Option<&GitRef>,
        expected_head: &CommitOid,
        kind: WorkspaceKind,
    ) -> impl Future<Output = Result<(), UseCaseError>> + Send;

    fn remove_workspace_files(
        &self,
        project_id: ProjectId,
        workspace_path: &Path,
    ) -> impl Future<Output = Result<(), UseCaseError>> + Send;

    fn resolve_ref_oid(
        &self,
        project_id: ProjectId,
        ref_name: &GitRef,
    ) -> impl Future<Output = Result<Option<CommitOid>, UseCaseError>> + Send;

    fn delete_ref(
        &self,
        project_id: ProjectId,
        ref_name: &GitRef,
    ) -> impl Future<Output = Result<(), UseCaseError>> + Send;
}

pub async fn reset_workspace<S, G>(
    store: &S,
    git_port: &G,
    project_id: ProjectId,
    workspace: &Workspace,
) -> Result<Workspace, UseCaseError>
where
    S: WorkspaceCommandStore,
    G: WorkspaceInfraPort,
{
    let expected_head = workspace
        .state
        .head_commit_oid()
        .cloned()
        .ok_or_else(|| UseCaseError::Internal("workspace missing head_commit_oid".into()))?;

    let now = Utc::now();
    let mut operation = GitOperation {
        id: GitOperationId::new(),
        project_id,
        entity: GitOperationEntityRef::Workspace(workspace.id),
        payload: OperationPayload::ResetWorkspace {
            workspace_id: workspace.id,
            ref_name: workspace.workspace_ref.clone(),
            expected_old_oid: workspace.state.head_commit_oid().cloned(),
            new_oid: expected_head.clone(),
        },
        status: GitOperationStatus::Planned,
        created_at: now,
        completed_at: None,
    };
    create_planned(store, store, &operation, project_id).await?;

    git_port
        .reset_worktree(
            project_id,
            &workspace.path,
            workspace.workspace_ref.as_ref(),
            &expected_head,
            workspace.kind,
        )
        .await?;

    let mut updated = workspace.clone();
    updated.mark_ready_with_head(expected_head, Utc::now());
    <S as WorkspaceRepository>::update(store, &updated)
        .await
        .map_err(UseCaseError::Repository)?;

    mark_applied(store, &mut operation).await?;

    Ok(updated)
}

pub async fn remove_workspace_full<S, G>(
    store: &S,
    git_port: &G,
    project_id: ProjectId,
    workspace: &Workspace,
) -> Result<Workspace, UseCaseError>
where
    S: WorkspaceCommandStore,
    G: WorkspaceInfraPort,
{
    let workspace = plan_workspace_removal(store, workspace).await?;

    if workspace.path.exists() {
        git_port
            .remove_workspace_files(project_id, &workspace.path)
            .await?;
    }

    if let Some(workspace_ref) = workspace.workspace_ref.as_ref() {
        let current_ref_oid = git_port.resolve_ref_oid(project_id, workspace_ref).await?;
        if let Some(expected_old_oid) = current_ref_oid {
            let now = Utc::now();
            let mut operation = GitOperation {
                id: GitOperationId::new(),
                project_id,
                entity: GitOperationEntityRef::Workspace(workspace.id),
                payload: OperationPayload::RemoveWorkspaceRef {
                    workspace_id: workspace.id,
                    ref_name: workspace_ref.clone(),
                    expected_old_oid,
                },
                status: GitOperationStatus::Planned,
                created_at: now,
                completed_at: None,
            };
            create_planned(store, store, &operation, project_id).await?;
            git_port.delete_ref(project_id, workspace_ref).await?;
            mark_applied(store, &mut operation).await?;
        }
    }

    finalize_workspace_removal(store, &workspace).await
}
