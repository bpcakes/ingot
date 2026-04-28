use std::path::Path;

use ingot_git::commands::git;

use crate::WorkspaceError;

pub async fn remove_workspace(
    repo_path: &Path,
    workspace_path: &Path,
) -> Result<(), WorkspaceError> {
    if !workspace_path.exists() {
        return Ok(());
    }

    let workspace_path = workspace_path.to_string_lossy().into_owned();
    git(
        repo_path,
        &["worktree", "remove", "--force", &workspace_path],
    )
    .await?;
    Ok(())
}
