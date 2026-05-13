use std::path::Path;

use ingot_domain::commit_oid::CommitOid;
use ingot_domain::git_ref::GitRef;
use ingot_domain::ids::ProjectId;
use ingot_domain::job::Job;
use ingot_domain::project::Project;
use ingot_domain::revision::ItemRevision;
use ingot_domain::workspace::{Workspace, WorkspaceKind};
use ingot_git::commands::FinalizeTargetRefOutcome;
use ingot_git::diff::changed_paths_between;
use ingot_git::project_repo::{
    CheckoutFinalizationStatus, ProjectRepoPaths, checkout_finalization_status,
    refresh_project_mirror_for_project, sync_checkout_to_commit,
};
use ingot_usecases::{
    UseCaseError, application::ApplicationInfraPort, dispatch::DispatchInfraPort,
    workspace::WorkspaceInfraPort,
};
use ingot_workspace::ensure_authoring_workspace_state;

use crate::ApplicationDatabase;
use crate::errors::{
    ensure_git_valid_target_ref_usecase, git_to_usecase_error, workspace_to_usecase_error,
};

#[derive(Clone)]
pub(crate) struct ApplicationInfra {
    db: ApplicationDatabase,
    state_root: std::path::PathBuf,
}

impl ApplicationInfra {
    pub(crate) fn new(db: ApplicationDatabase, state_root: std::path::PathBuf) -> Self {
        Self { db, state_root }
    }

    pub(crate) async fn refresh_project_mirror(
        &self,
        project: &Project,
    ) -> Result<ProjectRepoPaths, UseCaseError> {
        refresh_project_mirror_for_project(&self.db, self.state_root.as_path(), project)
            .await
            .map_err(|error| match error {
                ingot_git::project_repo::RefreshMirrorError::Repository(error) => {
                    UseCaseError::Repository(error)
                }
                ingot_git::project_repo::RefreshMirrorError::Git(error) => {
                    git_to_usecase_error(error)
                }
            })
    }

    pub(crate) async fn mirror_paths(
        &self,
        project_id: ProjectId,
    ) -> Result<ProjectRepoPaths, UseCaseError> {
        let project = self
            .db
            .get_project(project_id)
            .await
            .map_err(UseCaseError::Repository)?;
        self.refresh_project_mirror(&project).await
    }

    pub(crate) async fn resolve_project_ref_oid(
        &self,
        project_id: ProjectId,
        ref_name: &GitRef,
    ) -> Result<Option<CommitOid>, UseCaseError> {
        let paths = self.mirror_paths(project_id).await?;
        ingot_git::commands::resolve_ref_oid(paths.mirror_git_dir.as_path(), ref_name)
            .await
            .map_err(git_to_usecase_error)
    }

    pub(crate) async fn changed_paths_between(
        &self,
        project_id: ProjectId,
        base_commit_oid: &CommitOid,
        head_commit_oid: &CommitOid,
    ) -> Result<Vec<String>, UseCaseError> {
        let paths = self.mirror_paths(project_id).await?;
        changed_paths_between(
            paths.mirror_git_dir.as_path(),
            base_commit_oid,
            head_commit_oid,
        )
        .await
        .map_err(git_to_usecase_error)
    }

    pub(crate) async fn is_commit_reachable_from_any_ref(
        &self,
        project_id: ProjectId,
        commit_oid: &CommitOid,
    ) -> Result<bool, UseCaseError> {
        let paths = self.mirror_paths(project_id).await?;
        ingot_git::commands::is_commit_reachable_from_any_ref(
            paths.mirror_git_dir.as_path(),
            commit_oid,
        )
        .await
        .map_err(git_to_usecase_error)
    }

    pub(crate) async fn is_commit_reachable_from_project(
        &self,
        project: &Project,
        commit_oid: &CommitOid,
    ) -> Result<bool, UseCaseError> {
        let paths = self.refresh_project_mirror(project).await?;
        ingot_git::commands::is_commit_reachable_from_any_ref(
            paths.mirror_git_dir.as_path(),
            commit_oid,
        )
        .await
        .map_err(git_to_usecase_error)
    }

    pub(crate) async fn ensure_authoring_workspace(
        &self,
        project_id: ProjectId,
        revision: &ItemRevision,
        job: &Job,
        existing: Option<Workspace>,
    ) -> Result<Workspace, UseCaseError> {
        let paths = self.mirror_paths(project_id).await?;
        ensure_authoring_workspace_state(
            existing,
            project_id,
            paths.mirror_git_dir.as_path(),
            paths.worktree_root.as_path(),
            revision,
            job,
            chrono::Utc::now(),
        )
        .await
        .map_err(workspace_to_usecase_error)
    }

    pub(crate) async fn checkout_finalization_status(
        &self,
        project: &Project,
        target_ref: &GitRef,
        prepared_commit_oid: &CommitOid,
    ) -> Result<CheckoutFinalizationStatus, UseCaseError> {
        let paths = self.refresh_project_mirror(project).await?;
        checkout_finalization_status(
            &project.path,
            paths.mirror_git_dir.as_path(),
            target_ref,
            prepared_commit_oid,
        )
        .await
        .map_err(git_to_usecase_error)
    }

    pub(crate) async fn sync_checkout_to_prepared_commit(
        &self,
        project: &Project,
        target_ref: &GitRef,
        prepared_commit_oid: &CommitOid,
    ) -> Result<(), UseCaseError> {
        let paths = self.mirror_paths(project.id).await?;
        sync_checkout_to_commit(
            &project.path,
            paths.mirror_git_dir.as_path(),
            target_ref,
            prepared_commit_oid,
        )
        .await
        .map_err(git_to_usecase_error)
    }

    pub(crate) async fn finalize_target_ref(
        &self,
        project_id: ProjectId,
        ref_name: &GitRef,
        prepared_commit_oid: &CommitOid,
        expected_old_oid: &CommitOid,
    ) -> Result<FinalizeTargetRefOutcome, UseCaseError> {
        let paths = self.mirror_paths(project_id).await?;
        ingot_git::commands::finalize_target_ref(
            paths.mirror_git_dir.as_path(),
            ref_name,
            prepared_commit_oid,
            expected_old_oid,
        )
        .await
        .map_err(git_to_usecase_error)
    }

    pub(crate) async fn remove_workspace_path(
        &self,
        project_id: ProjectId,
        workspace_path: &Path,
    ) -> Result<(), UseCaseError> {
        let paths = self.mirror_paths(project_id).await?;
        ingot_workspace::remove_workspace(paths.mirror_git_dir.as_path(), workspace_path)
            .await
            .map_err(workspace_to_usecase_error)
    }

    async fn remove_workspace_with_ref_cleanup(
        &self,
        project_id: ProjectId,
        workspace: &Workspace,
    ) -> Result<(), UseCaseError> {
        self.remove_workspace_path(project_id, &workspace.path)
            .await?;
        if let Some(workspace_ref) = workspace.workspace_ref.as_ref() {
            let _ = self.delete_project_ref(project_id, workspace_ref).await;
        }
        Ok(())
    }

    async fn update_project_ref(
        &self,
        project_id: ProjectId,
        ref_name: &GitRef,
        commit_oid: &CommitOid,
    ) -> Result<(), UseCaseError> {
        let paths = self.mirror_paths(project_id).await?;
        ingot_git::commands::update_ref(paths.mirror_git_dir.as_path(), ref_name, commit_oid)
            .await
            .map_err(git_to_usecase_error)
    }

    async fn delete_project_ref(
        &self,
        project_id: ProjectId,
        ref_name: &GitRef,
    ) -> Result<(), UseCaseError> {
        let paths = self.mirror_paths(project_id).await?;
        ingot_git::commands::delete_ref(paths.mirror_git_dir.as_path(), ref_name)
            .await
            .map_err(git_to_usecase_error)
    }
}

impl DispatchInfraPort for ApplicationInfra {
    async fn resolve_ref_oid(
        &self,
        project_id: ProjectId,
        ref_name: &GitRef,
    ) -> Result<Option<CommitOid>, UseCaseError> {
        self.resolve_project_ref_oid(project_id, ref_name).await
    }

    async fn update_ref(
        &self,
        project_id: ProjectId,
        ref_name: &GitRef,
        commit_oid: &CommitOid,
    ) -> Result<(), UseCaseError> {
        self.update_project_ref(project_id, ref_name, commit_oid)
            .await
    }

    async fn delete_ref(
        &self,
        project_id: ProjectId,
        ref_name: &GitRef,
    ) -> Result<(), UseCaseError> {
        self.delete_project_ref(project_id, ref_name).await
    }

    async fn remove_workspace_files(
        &self,
        project_id: ProjectId,
        workspace: &Workspace,
    ) -> Result<(), UseCaseError> {
        self.remove_workspace_with_ref_cleanup(project_id, workspace)
            .await
    }

    async fn ensure_authoring_workspace(
        &self,
        project_id: ProjectId,
        revision: &ItemRevision,
        job: &Job,
        existing: Option<Workspace>,
    ) -> Result<Workspace, UseCaseError> {
        ApplicationInfra::ensure_authoring_workspace(self, project_id, revision, job, existing)
            .await
    }
}

impl ApplicationInfraPort for ApplicationInfra {
    async fn ensure_valid_target_ref(&self, target_ref: &str) -> Result<(), UseCaseError> {
        ensure_git_valid_target_ref_usecase(target_ref).await
    }

    async fn refresh_project_mirror(&self, project: &Project) -> Result<(), UseCaseError> {
        ApplicationInfra::refresh_project_mirror(self, project)
            .await
            .map(|_| ())
    }

    async fn resolve_project_ref_oid(
        &self,
        project_id: ProjectId,
        ref_name: &GitRef,
    ) -> Result<Option<CommitOid>, UseCaseError> {
        ApplicationInfra::resolve_project_ref_oid(self, project_id, ref_name).await
    }

    async fn is_commit_reachable_from_any_ref(
        &self,
        project_id: ProjectId,
        commit_oid: &CommitOid,
    ) -> Result<bool, UseCaseError> {
        ApplicationInfra::is_commit_reachable_from_any_ref(self, project_id, commit_oid).await
    }

    async fn is_commit_reachable_from_project(
        &self,
        project: &Project,
        commit_oid: &CommitOid,
    ) -> Result<bool, UseCaseError> {
        ApplicationInfra::is_commit_reachable_from_project(self, project, commit_oid).await
    }

    async fn changed_paths_between(
        &self,
        project_id: ProjectId,
        base_commit_oid: &CommitOid,
        head_commit_oid: &CommitOid,
    ) -> Result<Vec<String>, UseCaseError> {
        ApplicationInfra::changed_paths_between(self, project_id, base_commit_oid, head_commit_oid)
            .await
    }

    async fn remove_workspace_path(
        &self,
        project_id: ProjectId,
        workspace_path: &Path,
    ) -> Result<(), UseCaseError> {
        ApplicationInfra::remove_workspace_path(self, project_id, workspace_path).await
    }
}

impl WorkspaceInfraPort for ApplicationInfra {
    async fn reset_worktree(
        &self,
        project_id: ProjectId,
        workspace_path: &Path,
        workspace_ref: Option<&GitRef>,
        expected_head: &CommitOid,
        kind: WorkspaceKind,
    ) -> Result<(), UseCaseError> {
        let paths = self.mirror_paths(project_id).await?;
        match kind {
            WorkspaceKind::Authoring | WorkspaceKind::Integration => {
                ingot_git::commands::git(
                    workspace_path,
                    &["reset", "--hard", expected_head.as_str()],
                )
                .await
                .map_err(git_to_usecase_error)?;
                ingot_git::commands::git(workspace_path, &["clean", "-fd"])
                    .await
                    .map_err(git_to_usecase_error)?;
                if let Some(workspace_ref) = workspace_ref {
                    ingot_git::commands::git(
                        paths.mirror_git_dir.as_path(),
                        &["update-ref", workspace_ref.as_str(), expected_head.as_str()],
                    )
                    .await
                    .map_err(git_to_usecase_error)?;
                }
            }
            WorkspaceKind::Review => {
                ingot_workspace::provision_review_workspace(
                    paths.mirror_git_dir.as_path(),
                    workspace_path,
                    expected_head,
                )
                .await
                .map_err(workspace_to_usecase_error)?;
            }
        }
        Ok(())
    }

    async fn remove_workspace_files(
        &self,
        project_id: ProjectId,
        workspace_path: &Path,
    ) -> Result<(), UseCaseError> {
        self.remove_workspace_path(project_id, workspace_path).await
    }

    async fn resolve_ref_oid(
        &self,
        project_id: ProjectId,
        ref_name: &GitRef,
    ) -> Result<Option<CommitOid>, UseCaseError> {
        self.resolve_project_ref_oid(project_id, ref_name).await
    }

    async fn delete_ref(
        &self,
        project_id: ProjectId,
        ref_name: &GitRef,
    ) -> Result<(), UseCaseError> {
        self.delete_project_ref(project_id, ref_name).await
    }
}
