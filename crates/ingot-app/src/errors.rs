use ingot_domain::ports::RepositoryError;
use ingot_git::commands::{GitCommandError, check_ref_format};
use ingot_usecases::{UseCaseError, UseCaseInfraError};
use ingot_workspace::WorkspaceError;

pub(crate) fn workspace_to_usecase_error(error: WorkspaceError) -> UseCaseError {
    match error {
        error @ WorkspaceError::Busy => UseCaseInfraError::workspace_busy(error).into(),
        error @ WorkspaceError::MissingInputHeadCommitOid => {
            UseCaseInfraError::workspace_invalid_state(error).into()
        }
        error @ (WorkspaceError::WorkspaceRefMismatch { .. }
        | WorkspaceError::WorkspaceHeadMismatch { .. }) => {
            UseCaseInfraError::workspace_state_mismatch(error).into()
        }
        other => UseCaseInfraError::external("workspace", other).into(),
    }
}

pub(crate) fn git_to_usecase_error(error: GitCommandError) -> UseCaseError {
    UseCaseInfraError::git(error).into()
}

pub(crate) fn repo_to_item_usecase(error: RepositoryError) -> UseCaseError {
    match error {
        RepositoryError::NotFound => UseCaseError::ItemNotFound,
        other => UseCaseError::Repository(other),
    }
}

pub(crate) fn repo_to_project_usecase(error: RepositoryError) -> UseCaseError {
    match error {
        RepositoryError::NotFound => UseCaseError::ProjectNotFound,
        other => UseCaseError::Repository(other),
    }
}

pub(crate) async fn ensure_git_valid_target_ref_usecase(
    target_ref: &str,
) -> Result<(), UseCaseError> {
    match check_ref_format(target_ref)
        .await
        .map_err(git_to_usecase_error)?
    {
        true => Ok(()),
        false => Err(UseCaseError::InvalidTargetRef(target_ref.into())),
    }
}
