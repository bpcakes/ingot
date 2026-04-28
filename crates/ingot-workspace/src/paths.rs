use std::path::{Path, PathBuf};

use ingot_domain::ids::ProjectId;

pub fn workspace_root_path(repo_path: &Path) -> PathBuf {
    let parent = repo_path.parent().unwrap_or(repo_path);
    parent.join(".ingot-workspaces")
}

pub fn managed_workspace_root_path(state_root: &Path, project_id: ProjectId) -> PathBuf {
    state_root.join("worktrees").join(project_id.to_string())
}
