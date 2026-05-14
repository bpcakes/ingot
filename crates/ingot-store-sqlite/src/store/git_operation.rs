use std::collections::HashSet;

use ingot_domain::git_operation::{GitOperation, GitOperationEntityRef, GitOperationWire};
use ingot_domain::git_ref::GitRef;
use ingot_domain::ids::ConvergenceId;
use ingot_domain::ports::{ConflictKind, GitOperationRepository, RepositoryError};
use sqlx::QueryBuilder;
use sqlx::sqlite::SqliteRow;

use super::helpers::{
    db_err, db_text, db_write_err, ensure_rows_affected, json_err, map_optional_row,
    optional_db_text, row_get, row_get_optional_json,
};
use crate::db::Database;

impl Database {
    pub async fn create_git_operation(
        &self,
        operation: &GitOperation,
    ) -> Result<(), RepositoryError> {
        let wire = GitOperationWire::from(operation);
        sqlx::query(
            "INSERT INTO git_operations (
                id, project_id, operation_kind, entity_type, entity_id, workspace_id, ref_name,
                expected_old_oid, new_oid, commit_oid, status, metadata, created_at, completed_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(db_text(wire.id))
        .bind(db_text(wire.project_id))
        .bind(db_text(wire.operation_kind))
        .bind(db_text(wire.entity_type))
        .bind(&wire.entity_id)
        .bind(optional_db_text(wire.workspace_id))
        .bind(optional_db_text(wire.ref_name.clone()))
        .bind(optional_db_text(wire.expected_old_oid.clone()))
        .bind(optional_db_text(wire.new_oid.clone()))
        .bind(optional_db_text(wire.commit_oid.clone()))
        .bind(db_text(wire.status))
        .bind(
            wire.metadata
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(json_err)?,
        )
        .bind(wire.created_at)
        .bind(wire.completed_at)
        .execute(&self.pool)
        .await
        .map_err(db_write_err)?;

        Ok(())
    }

    pub async fn update_git_operation(
        &self,
        operation: &GitOperation,
    ) -> Result<(), RepositoryError> {
        let wire = GitOperationWire::from(operation);
        let result = sqlx::query(
            "UPDATE git_operations
             SET workspace_id = ?, ref_name = ?, expected_old_oid = ?, new_oid = ?, commit_oid = ?,
                 status = ?, metadata = ?, completed_at = ?
             WHERE id = ?",
        )
        .bind(optional_db_text(wire.workspace_id))
        .bind(optional_db_text(wire.ref_name.clone()))
        .bind(optional_db_text(wire.expected_old_oid.clone()))
        .bind(optional_db_text(wire.new_oid.clone()))
        .bind(optional_db_text(wire.commit_oid.clone()))
        .bind(db_text(wire.status))
        .bind(
            wire.metadata
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(json_err)?,
        )
        .bind(wire.completed_at)
        .bind(db_text(wire.id))
        .execute(&self.pool)
        .await
        .map_err(db_write_err)?;

        ensure_rows_affected(result)
    }

    pub async fn list_unresolved_git_operations(
        &self,
    ) -> Result<Vec<GitOperation>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, project_id, operation_kind, entity_type, entity_id, workspace_id, ref_name,
                    expected_old_oid, new_oid, commit_oid, status, metadata, created_at, completed_at
             FROM git_operations
             WHERE status IN ('planned', 'applied')
             ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.iter().map(map_git_operation).collect()
    }

    pub async fn find_unresolved_finalize_for_convergence(
        &self,
        convergence_id: ConvergenceId,
    ) -> Result<Option<GitOperation>, RepositoryError> {
        let row = sqlx::query(
            "SELECT id, project_id, operation_kind, entity_type, entity_id, workspace_id, ref_name,
                    expected_old_oid, new_oid, commit_oid, status, metadata, created_at, completed_at
             FROM git_operations
             WHERE operation_kind = 'finalize_target_ref'
               AND entity_type = 'convergence'
               AND entity_id = ?
               AND status IN ('planned', 'applied')
             ORDER BY created_at ASC
             LIMIT 1",
        )
        .bind(db_text(convergence_id))
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;

        map_optional_row(row, map_git_operation)
    }

    pub async fn list_latest_failed_prepare_for_convergences(
        &self,
        convergence_ids: &[ConvergenceId],
    ) -> Result<Vec<GitOperation>, RepositoryError> {
        // Stay below SQLite builds that still use a 999 bind-parameter limit.
        const SQLITE_BIND_CHUNK_SIZE: usize = 900;

        if convergence_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut requested = HashSet::new();
        let convergence_ids = convergence_ids
            .iter()
            .copied()
            .filter(|convergence_id| requested.insert(*convergence_id))
            .collect::<Vec<_>>();
        let mut seen = HashSet::new();
        let mut operations = Vec::new();
        for chunk in convergence_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
            for operation in self
                .list_latest_failed_prepare_for_convergence_chunk(chunk)
                .await?
            {
                let GitOperationEntityRef::Convergence(convergence_id) = operation.entity else {
                    continue;
                };
                // Each chunk dedups the latest operation for each convergence because a
                // convergence may have history; this final seen set is a defensive backstop.
                if seen.insert(convergence_id) {
                    operations.push(operation);
                }
            }
        }

        Ok(operations)
    }

    async fn list_latest_failed_prepare_for_convergence_chunk(
        &self,
        convergence_ids: &[ConvergenceId],
    ) -> Result<Vec<GitOperation>, RepositoryError> {
        let mut query = QueryBuilder::new(
            "SELECT id, project_id, operation_kind, entity_type, entity_id, workspace_id, ref_name,
                    expected_old_oid, new_oid, commit_oid, status, metadata, created_at, completed_at
             FROM git_operations
             WHERE operation_kind = 'prepare_convergence_commit'
               AND entity_type = 'convergence'
               AND status = 'failed'
               AND entity_id IN (",
        );
        let mut separated = query.separated(", ");
        for convergence_id in convergence_ids {
            separated.push_bind(convergence_id.to_string());
        }
        separated.push_unseparated(")");
        query.push(" ORDER BY entity_id ASC, created_at DESC, id DESC");

        let rows = query.build().fetch_all(&self.pool).await.map_err(db_err)?;
        let mut operations = Vec::new();
        let mut seen = HashSet::new();
        for row in rows {
            let operation = map_git_operation(&row)?;
            let GitOperationEntityRef::Convergence(convergence_id) = operation.entity else {
                continue;
            };
            if seen.insert(convergence_id) {
                operations.push(operation);
            }
        }

        Ok(operations)
    }

    pub async fn delete_investigation_ref_git_operations(
        &self,
        ref_name: &GitRef,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "DELETE FROM git_operations WHERE operation_kind = 'create_investigation_ref' AND ref_name = ?",
        )
        .bind(db_text(ref_name))
        .execute(&self.pool)
        .await
        .map_err(db_write_err)?;

        Ok(())
    }
}

impl GitOperationRepository for Database {
    async fn create(&self, operation: &GitOperation) -> Result<(), RepositoryError> {
        self.create_git_operation(operation).await
    }
    async fn update(&self, operation: &GitOperation) -> Result<(), RepositoryError> {
        self.update_git_operation(operation).await
    }
    async fn find_unresolved(&self) -> Result<Vec<GitOperation>, RepositoryError> {
        self.list_unresolved_git_operations().await
    }
    async fn find_unresolved_finalize_for_convergence(
        &self,
        convergence_id: ConvergenceId,
    ) -> Result<Option<GitOperation>, RepositoryError> {
        Database::find_unresolved_finalize_for_convergence(self, convergence_id).await
    }
    async fn delete_investigation_ref_operations(
        &self,
        ref_name: &GitRef,
    ) -> Result<(), RepositoryError> {
        self.delete_investigation_ref_git_operations(ref_name).await
    }
}

fn map_git_operation(row: &SqliteRow) -> Result<GitOperation, RepositoryError> {
    let wire = GitOperationWire {
        id: row_get(row, "id")?,
        project_id: row_get(row, "project_id")?,
        operation_kind: row_get(row, "operation_kind")?,
        entity_type: row_get(row, "entity_type")?,
        entity_id: row_get(row, "entity_id")?,
        workspace_id: row_get(row, "workspace_id")?,
        ref_name: row_get(row, "ref_name")?,
        expected_old_oid: row_get(row, "expected_old_oid")?,
        new_oid: row_get(row, "new_oid")?,
        commit_oid: row_get(row, "commit_oid")?,
        status: row_get(row, "status")?,
        metadata: row_get_optional_json(row, "metadata")?,
        created_at: row_get(row, "created_at")?,
        completed_at: row_get(row, "completed_at")?,
    };
    GitOperation::try_from(wire).map_err(|e| {
        RepositoryError::Conflict(ConflictKind::Other(format!("invalid git operation: {e}")))
    })
}
