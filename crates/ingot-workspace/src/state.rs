use std::path::Path;

use chrono::{DateTime, Utc};
use ingot_domain::commit_oid::CommitOid;
use ingot_domain::git_ref::GitRef;
use ingot_domain::ids::ProjectId;
use ingot_domain::job::Job;
use ingot_domain::revision::ItemRevision;
use ingot_domain::workspace::{
    RetentionPolicy, Workspace, WorkspaceCommitState, WorkspaceKind, WorkspaceState,
    WorkspaceStatus, WorkspaceStrategy,
};

use crate::WorkspaceError;
use crate::provision::provision_authoring_workspace;

pub async fn ensure_authoring_workspace_state(
    existing: Option<Workspace>,
    project_id: ProjectId,
    repo_path: &Path,
    workspace_root: &Path,
    revision: &ItemRevision,
    job: &Job,
    now: DateTime<Utc>,
) -> Result<Workspace, WorkspaceError> {
    fn resolve_base_commit_oid(
        workspace: Option<&Workspace>,
        revision: &ItemRevision,
        job: &Job,
        expected_head_commit_oid: &CommitOid,
    ) -> CommitOid {
        workspace
            .and_then(|workspace| workspace.state.base_commit_oid().map(ToOwned::to_owned))
            .or_else(|| revision.seed.seed_commit_oid().map(ToOwned::to_owned))
            .or_else(|| job.job_input.head_commit_oid().map(ToOwned::to_owned))
            .unwrap_or_else(|| expected_head_commit_oid.clone())
    }

    let expected_head_commit_oid = job
        .job_input
        .head_commit_oid()
        .map(ToOwned::to_owned)
        .ok_or(WorkspaceError::MissingInputHeadCommitOid)?;
    let workspace_id = existing
        .as_ref()
        .map(|workspace| workspace.id)
        .unwrap_or_default();
    let workspace_path = existing
        .as_ref()
        .map(|workspace| workspace.path.clone())
        .unwrap_or_else(|| workspace_root.join(workspace_id.to_string()));
    let workspace_ref = existing
        .as_ref()
        .and_then(|workspace| workspace.workspace_ref.clone())
        .unwrap_or_else(|| GitRef::new(format!("refs/ingot/workspaces/{workspace_id}")));

    if let Some(mut workspace) = existing {
        if workspace.state.status() == WorkspaceStatus::Busy {
            return Err(WorkspaceError::Busy);
        }

        let provisioned = provision_authoring_workspace(
            repo_path,
            &workspace_path,
            &workspace_ref,
            &expected_head_commit_oid,
        )
        .await?;
        workspace.path = provisioned.workspace_path;
        workspace.target_ref = Some(revision.target_ref.clone());
        workspace.workspace_ref = Some(provisioned.workspace_ref);
        workspace.mark_ready(
            WorkspaceCommitState::new(
                resolve_base_commit_oid(Some(&workspace), revision, job, &expected_head_commit_oid),
                provisioned.head_commit_oid,
            ),
            now,
        );
        Ok(workspace)
    } else {
        let provisioned = provision_authoring_workspace(
            repo_path,
            &workspace_path,
            &workspace_ref,
            &expected_head_commit_oid,
        )
        .await?;

        Ok(Workspace {
            id: workspace_id,
            project_id,
            kind: WorkspaceKind::Authoring,
            strategy: WorkspaceStrategy::Worktree,
            path: provisioned.workspace_path,
            created_for_revision_id: Some(revision.id),
            parent_workspace_id: None,
            target_ref: Some(revision.target_ref.clone()),
            workspace_ref: Some(provisioned.workspace_ref),
            retention_policy: RetentionPolicy::Persistent,
            state: WorkspaceState::Ready {
                commits: WorkspaceCommitState::new(
                    resolve_base_commit_oid(None, revision, job, &expected_head_commit_oid),
                    provisioned.head_commit_oid,
                ),
            },
            created_at: now,
            updated_at: now,
        })
    }
}
