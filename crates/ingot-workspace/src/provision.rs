use std::path::{Path, PathBuf};

use ingot_domain::commit_oid::CommitOid;
use ingot_domain::git_ref::GitRef;
use ingot_git::commands::{current_head_ref, git, head_oid, resolve_ref_oid};

use crate::WorkspaceError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionedAuthoringWorkspace {
    pub workspace_path: PathBuf,
    pub workspace_ref: GitRef,
    pub head_commit_oid: CommitOid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionedReviewWorkspace {
    pub workspace_path: PathBuf,
    pub head_commit_oid: CommitOid,
}

pub async fn provision_authoring_workspace(
    repo_path: &Path,
    workspace_path: &Path,
    workspace_ref: &GitRef,
    expected_head_oid: &CommitOid,
) -> Result<ProvisionedAuthoringWorkspace, WorkspaceError> {
    if let Some(parent) = workspace_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let current_ref = resolve_ref_oid(repo_path, workspace_ref).await?;
    if current_ref.as_ref() != Some(expected_head_oid) {
        git(
            repo_path,
            &[
                "update-ref",
                workspace_ref.as_str(),
                expected_head_oid.as_str(),
            ],
        )
        .await?;
    }

    if !workspace_path.exists() {
        let workspace_path = workspace_path.to_string_lossy().into_owned();
        git(
            repo_path,
            &[
                "worktree",
                "add",
                "--detach",
                &workspace_path,
                workspace_ref.as_str(),
            ],
        )
        .await?;
    } else {
        reset_existing_worktree(workspace_path, expected_head_oid).await?;
    }

    verify_authoring_workspace(repo_path, workspace_path, workspace_ref, expected_head_oid).await
}

pub async fn verify_authoring_workspace(
    repo_path: &Path,
    workspace_path: &Path,
    workspace_ref: &GitRef,
    expected_head_oid: &CommitOid,
) -> Result<ProvisionedAuthoringWorkspace, WorkspaceError> {
    let actual_ref = resolve_ref_oid(repo_path, workspace_ref).await?;
    if actual_ref.as_ref() != Some(expected_head_oid) {
        return Err(WorkspaceError::WorkspaceRefMismatch {
            expected: workspace_ref.clone(),
            actual: current_head_ref(workspace_path).await?,
        });
    }

    let actual_head = head_oid(workspace_path).await?;
    if actual_head != *expected_head_oid {
        return Err(WorkspaceError::WorkspaceHeadMismatch {
            expected: expected_head_oid.clone(),
            actual: actual_head,
        });
    }

    Ok(ProvisionedAuthoringWorkspace {
        workspace_path: workspace_path.to_path_buf(),
        workspace_ref: workspace_ref.clone(),
        head_commit_oid: expected_head_oid.clone(),
    })
}

pub async fn provision_review_workspace(
    repo_path: &Path,
    workspace_path: &Path,
    expected_head_oid: &CommitOid,
) -> Result<ProvisionedReviewWorkspace, WorkspaceError> {
    if let Some(parent) = workspace_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    if workspace_path.exists() {
        let workspace_path = workspace_path.to_string_lossy().into_owned();
        git(
            repo_path,
            &["worktree", "remove", "--force", &workspace_path],
        )
        .await?;
    }

    let workspace_path_arg = workspace_path.to_string_lossy().into_owned();
    git(
        repo_path,
        &[
            "worktree",
            "add",
            "--detach",
            &workspace_path_arg,
            expected_head_oid.as_str(),
        ],
    )
    .await?;

    verify_review_workspace(workspace_path, expected_head_oid).await
}

pub async fn provision_integration_workspace(
    repo_path: &Path,
    workspace_path: &Path,
    workspace_ref: &GitRef,
    expected_head_oid: &CommitOid,
) -> Result<ProvisionedAuthoringWorkspace, WorkspaceError> {
    provision_authoring_workspace(repo_path, workspace_path, workspace_ref, expected_head_oid).await
}

async fn reset_existing_worktree(
    workspace_path: &Path,
    expected_head_oid: &CommitOid,
) -> Result<(), WorkspaceError> {
    git(
        workspace_path,
        &[
            "checkout",
            "--detach",
            "--force",
            expected_head_oid.as_str(),
        ],
    )
    .await?;
    git(
        workspace_path,
        &["reset", "--hard", expected_head_oid.as_str()],
    )
    .await?;
    git(workspace_path, &["clean", "-fd"]).await?;
    Ok(())
}

pub async fn verify_review_workspace(
    workspace_path: &Path,
    expected_head_oid: &CommitOid,
) -> Result<ProvisionedReviewWorkspace, WorkspaceError> {
    let actual_head = head_oid(workspace_path).await?;
    if actual_head != *expected_head_oid {
        return Err(WorkspaceError::WorkspaceHeadMismatch {
            expected: expected_head_oid.clone(),
            actual: actual_head,
        });
    }

    Ok(ProvisionedReviewWorkspace {
        workspace_path: workspace_path.to_path_buf(),
        head_commit_oid: expected_head_oid.clone(),
    })
}
