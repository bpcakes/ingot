use std::path::PathBuf;

use ingot_domain::ids::{ItemId, ItemRevisionId, ProjectId, WorkspaceId};
use ingot_domain::ports::{RepositoryError, WorkspaceRepository};
use ingot_domain::workspace::{Workspace, WorkspaceCommitState, WorkspaceState, WorkspaceStatus};
use sqlx::sqlite::SqliteRow;

use super::helpers::{
    db_err, db_text, db_write_err, ensure_rows_affected, map_optional_row, optional_db_text,
    required_row, row_get,
};
use crate::db::Database;

impl Database {
    pub async fn get_workspace(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Workspace, RepositoryError> {
        let row = sqlx::query("SELECT * FROM workspaces WHERE id = ?")
            .bind(db_text(workspace_id))
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;

        required_row(row, map_workspace)
    }

    pub async fn create_workspace(&self, workspace: &Workspace) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO workspaces (
                id, project_id, kind, strategy, path, created_for_revision_id, parent_workspace_id,
                target_ref, workspace_ref, base_commit_oid, head_commit_oid, retention_policy,
                status, current_job_id, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(db_text(workspace.id))
        .bind(db_text(workspace.project_id))
        .bind(db_text(workspace.kind))
        .bind(db_text(workspace.strategy))
        .bind(workspace.path.to_string_lossy().as_ref())
        .bind(optional_db_text(workspace.created_for_revision_id))
        .bind(optional_db_text(workspace.parent_workspace_id))
        .bind(optional_db_text(workspace.target_ref.as_ref()))
        .bind(optional_db_text(workspace.workspace_ref.as_ref()))
        .bind(optional_db_text(workspace.state.base_commit_oid().cloned()))
        .bind(optional_db_text(workspace.state.head_commit_oid().cloned()))
        .bind(db_text(workspace.retention_policy))
        .bind(db_text(workspace.state.status()))
        .bind(optional_db_text(workspace.state.current_job_id()))
        .bind(workspace.created_at)
        .bind(workspace.updated_at)
        .execute(&self.pool)
        .await
        .map_err(db_write_err)?;

        Ok(())
    }

    pub async fn update_workspace(&self, workspace: &Workspace) -> Result<(), RepositoryError> {
        let result = sqlx::query(
            "UPDATE workspaces
             SET path = ?, target_ref = ?, workspace_ref = ?, base_commit_oid = ?, head_commit_oid = ?,
                 retention_policy = ?, status = ?, current_job_id = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(workspace.path.to_string_lossy().as_ref())
        .bind(optional_db_text(workspace.target_ref.as_ref()))
        .bind(optional_db_text(workspace.workspace_ref.as_ref()))
        .bind(optional_db_text(workspace.state.base_commit_oid().cloned()))
        .bind(optional_db_text(workspace.state.head_commit_oid().cloned()))
        .bind(db_text(workspace.retention_policy))
        .bind(db_text(workspace.state.status()))
        .bind(optional_db_text(workspace.state.current_job_id()))
        .bind(workspace.updated_at)
        .bind(db_text(workspace.id))
        .execute(&self.pool)
        .await
        .map_err(db_write_err)?;

        ensure_rows_affected(result)
    }

    pub async fn find_authoring_workspace_for_revision(
        &self,
        revision_id: ItemRevisionId,
    ) -> Result<Option<Workspace>, RepositoryError> {
        let row = sqlx::query(
            "SELECT *
             FROM workspaces
             WHERE created_for_revision_id = ?
               AND kind = 'authoring'
             ORDER BY created_at DESC
             LIMIT 1",
        )
        .bind(db_text(revision_id))
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;

        map_optional_row(row, map_workspace)
    }

    pub async fn list_workspaces_by_item(
        &self,
        item_id: ItemId,
    ) -> Result<Vec<Workspace>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT w.*
             FROM workspaces w
             JOIN item_revisions r ON r.id = w.created_for_revision_id
             WHERE r.item_id = ?
             ORDER BY w.created_at DESC",
        )
        .bind(db_text(item_id))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.iter().map(map_workspace).collect()
    }

    pub async fn list_workspaces_by_project(
        &self,
        project_id: ProjectId,
    ) -> Result<Vec<Workspace>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT *
             FROM workspaces
             WHERE project_id = ?
             ORDER BY created_at DESC",
        )
        .bind(db_text(project_id))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.iter().map(map_workspace).collect()
    }

    pub async fn delete_workspace(&self, workspace_id: WorkspaceId) -> Result<(), RepositoryError> {
        let result = sqlx::query("DELETE FROM workspaces WHERE id = ?")
            .bind(db_text(workspace_id))
            .execute(&self.pool)
            .await
            .map_err(db_write_err)?;

        ensure_rows_affected(result)
    }
}

impl WorkspaceRepository for Database {
    async fn list_by_project(
        &self,
        project_id: ProjectId,
    ) -> Result<Vec<Workspace>, RepositoryError> {
        self.list_workspaces_by_project(project_id).await
    }
    async fn get(&self, id: WorkspaceId) -> Result<Workspace, RepositoryError> {
        self.get_workspace(id).await
    }
    async fn create(&self, workspace: &Workspace) -> Result<(), RepositoryError> {
        self.create_workspace(workspace).await
    }
    async fn update(&self, workspace: &Workspace) -> Result<(), RepositoryError> {
        self.update_workspace(workspace).await
    }
    async fn find_authoring_for_revision(
        &self,
        revision_id: ItemRevisionId,
    ) -> Result<Option<Workspace>, RepositoryError> {
        self.find_authoring_workspace_for_revision(revision_id)
            .await
    }
    async fn list_by_item(&self, item_id: ItemId) -> Result<Vec<Workspace>, RepositoryError> {
        self.list_workspaces_by_item(item_id).await
    }
    async fn delete(&self, id: WorkspaceId) -> Result<(), RepositoryError> {
        self.delete_workspace(id).await
    }
}

fn map_workspace(row: &SqliteRow) -> Result<Workspace, RepositoryError> {
    let status: WorkspaceStatus = row_get(row, "status")?;
    let current_job_id = row_get(row, "current_job_id")?;
    let state = WorkspaceState::from_parts(
        status,
        WorkspaceCommitState::from_option_parts(
            row_get(row, "base_commit_oid")?,
            row_get(row, "head_commit_oid")?,
        ),
        current_job_id,
    )
    .map_err(|error| db_err(std::io::Error::other(error)))?;

    Ok(Workspace {
        id: row_get(row, "id")?,
        project_id: row_get(row, "project_id")?,
        kind: row_get(row, "kind")?,
        strategy: row_get(row, "strategy")?,
        path: PathBuf::from(row_get::<String>(row, "path")?),
        created_for_revision_id: row_get(row, "created_for_revision_id")?,
        parent_workspace_id: row_get(row, "parent_workspace_id")?,
        target_ref: row_get(row, "target_ref")?,
        workspace_ref: row_get(row, "workspace_ref")?,
        retention_policy: row_get(row, "retention_policy")?,
        created_at: row_get(row, "created_at")?,
        updated_at: row_get(row, "updated_at")?,
        state,
    })
}
