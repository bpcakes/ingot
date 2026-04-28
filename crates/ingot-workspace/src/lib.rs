mod cleanup;
mod paths;
mod provision;
mod state;

use ingot_domain::commit_oid::CommitOid;
use ingot_domain::git_ref::GitRef;
use ingot_git::commands::GitCommandError;

pub use cleanup::remove_workspace;
pub use paths::{managed_workspace_root_path, workspace_root_path};
pub use provision::{
    ProvisionedAuthoringWorkspace, ProvisionedReviewWorkspace, provision_authoring_workspace,
    provision_integration_workspace, provision_review_workspace, verify_authoring_workspace,
    verify_review_workspace,
};
pub use state::ensure_authoring_workspace_state;

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("git error: {0}")]
    Git(#[from] GitCommandError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("authoring jobs require a job_input head")]
    MissingInputHeadCommitOid,
    #[error("authoring workspace is already busy")]
    Busy,
    #[error("workspace ref mismatch: expected {expected}, got {actual:?}")]
    WorkspaceRefMismatch {
        expected: GitRef,
        actual: Option<GitRef>,
    },
    #[error("workspace head mismatch: expected {expected}, got {actual}")]
    WorkspaceHeadMismatch {
        expected: CommitOid,
        actual: CommitOid,
    },
}

#[cfg(test)]
mod tests {
    use super::{
        provision_authoring_workspace, provision_integration_workspace, verify_authoring_workspace,
    };
    use ingot_domain::commit_oid::CommitOid;
    use ingot_domain::git_ref::GitRef;
    use ingot_git::commands::head_oid;
    use ingot_test_support::git::{
        git_output, run_git as git_sync, temp_git_repo, unique_temp_path,
    };

    #[tokio::test]
    async fn provision_authoring_workspace_creates_worktree_and_anchor_ref() {
        let repo = temp_git_repo("ingot-workspace");
        let expected_head = CommitOid::new(git_output(&repo, &["rev-parse", "HEAD"]));
        let workspace_path = unique_temp_path("ingot-workspace");

        let provisioned = provision_authoring_workspace(
            &repo,
            &workspace_path,
            &GitRef::new("refs/ingot/workspaces/wrk_test"),
            &expected_head,
        )
        .await
        .expect("provision workspace");

        assert_eq!(provisioned.head_commit_oid, expected_head);
        assert!(workspace_path.exists(), "workspace path should exist");

        verify_authoring_workspace(
            &repo,
            &workspace_path,
            &GitRef::new("refs/ingot/workspaces/wrk_test"),
            &provisioned.head_commit_oid,
        )
        .await
        .expect("verify provisioned workspace");
    }

    #[tokio::test]
    async fn provision_authoring_workspace_reuses_existing_worktree() {
        let repo = temp_git_repo("ingot-workspace");
        let expected_head = CommitOid::new(git_output(&repo, &["rev-parse", "HEAD"]));
        let workspace_path = unique_temp_path("ingot-workspace");

        provision_authoring_workspace(
            &repo,
            &workspace_path,
            &GitRef::new("refs/ingot/workspaces/wrk_test"),
            &expected_head,
        )
        .await
        .expect("first provision");

        provision_authoring_workspace(
            &repo,
            &workspace_path,
            &GitRef::new("refs/ingot/workspaces/wrk_test"),
            &expected_head,
        )
        .await
        .expect("second provision");

        verify_authoring_workspace(
            &repo,
            &workspace_path,
            &GitRef::new("refs/ingot/workspaces/wrk_test"),
            &expected_head,
        )
        .await
        .expect("workspace should still verify after reprovision");
    }

    #[tokio::test]
    async fn provision_authoring_workspace_resets_existing_worktree_to_expected_head() {
        let repo = temp_git_repo("ingot-workspace");
        let base_head = CommitOid::new(git_output(&repo, &["rev-parse", "HEAD"]));
        let workspace_path = unique_temp_path("ingot-workspace");
        let workspace_ref = "refs/ingot/workspaces/wrk_test";

        provision_authoring_workspace(
            &repo,
            &workspace_path,
            &GitRef::new(workspace_ref),
            &base_head,
        )
        .await
        .expect("first provision");

        std::fs::write(repo.join("tracked.txt"), "next").expect("write tracked");
        git_sync(&repo, &["add", "tracked.txt"]);
        git_sync(&repo, &["commit", "-m", "next"]);
        let next_head = head_oid(&repo).await.expect("next head").into_inner();

        git_sync(&workspace_path, &["checkout", &next_head]);

        provision_authoring_workspace(
            &repo,
            &workspace_path,
            &GitRef::new(workspace_ref),
            &base_head,
        )
        .await
        .expect("re-provision drifted workspace");

        assert_eq!(
            head_oid(&workspace_path)
                .await
                .expect("workspace head")
                .into_inner(),
            base_head.as_str()
        );
    }

    #[tokio::test]
    async fn provision_authoring_workspace_detaches_before_resetting_branch_attached_worktree() {
        let repo = temp_git_repo("ingot-workspace");
        let base_head = CommitOid::new(git_output(&repo, &["rev-parse", "HEAD"]));
        let workspace_path = unique_temp_path("ingot-workspace");
        let workspace_ref = "refs/ingot/workspaces/wrk_test";

        provision_authoring_workspace(
            &repo,
            &workspace_path,
            &GitRef::new(workspace_ref),
            &base_head,
        )
        .await
        .expect("first provision");

        std::fs::write(repo.join("tracked.txt"), "next").expect("write tracked");
        git_sync(&repo, &["add", "tracked.txt"]);
        git_sync(&repo, &["commit", "-m", "next"]);
        let next_head = head_oid(&repo).await.expect("next head").into_inner();

        git_sync(&repo, &["branch", "feature/drift", &next_head]);
        git_sync(&workspace_path, &["checkout", "feature/drift"]);

        provision_authoring_workspace(
            &repo,
            &workspace_path,
            &GitRef::new(workspace_ref),
            &base_head,
        )
        .await
        .expect("re-provision drifted workspace");

        assert_eq!(
            head_oid(&workspace_path)
                .await
                .expect("workspace head")
                .into_inner(),
            base_head.as_str()
        );
        assert_eq!(
            git_output(&repo, &["rev-parse", "refs/heads/feature/drift"]),
            next_head
        );
    }

    #[tokio::test]
    async fn provision_integration_workspace_resets_existing_worktree_to_expected_head() {
        let repo = temp_git_repo("ingot-workspace");
        let base_head = CommitOid::new(git_output(&repo, &["rev-parse", "HEAD"]));
        let workspace_path = unique_temp_path("ingot-workspace");
        let workspace_ref = "refs/ingot/workspaces/wrk_integration";

        provision_integration_workspace(
            &repo,
            &workspace_path,
            &GitRef::new(workspace_ref),
            &base_head,
        )
        .await
        .expect("first provision");

        std::fs::write(repo.join("tracked.txt"), "next").expect("write tracked");
        git_sync(&repo, &["add", "tracked.txt"]);
        git_sync(&repo, &["commit", "-m", "next"]);
        let next_head = head_oid(&repo).await.expect("next head").into_inner();

        git_sync(&workspace_path, &["checkout", &next_head]);

        provision_integration_workspace(
            &repo,
            &workspace_path,
            &GitRef::new(workspace_ref),
            &base_head,
        )
        .await
        .expect("re-provision drifted workspace");

        assert_eq!(
            head_oid(&workspace_path)
                .await
                .expect("workspace head")
                .into_inner(),
            base_head.as_str()
        );
    }
}
