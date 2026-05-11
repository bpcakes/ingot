use ingot_domain::ports::{
    InvalidatePreparedConvergenceMutation, InvalidatePreparedConvergenceRepository, RepositoryError,
};

use super::helpers::{
    db_err, db_text, db_write_err, ensure_rows_affected, json_err, optional_db_text,
};
use crate::db::Database;

impl Database {
    pub async fn apply_invalidate_prepared_convergence(
        &self,
        mutation: InvalidatePreparedConvergenceMutation,
    ) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;

        // 1. Update convergence (mark as failed)
        let convergence = &mutation.convergence;
        let state = &convergence.state;
        let result = sqlx::query(
            "UPDATE convergences
             SET integration_workspace_id = ?, source_head_commit_oid = ?, target_ref = ?,
                 strategy = ?, status = ?, input_target_commit_oid = ?,
                 prepared_commit_oid = ?, final_target_commit_oid = ?,
                 checkout_adoption_state = ?, checkout_adoption_message = ?,
                 checkout_adoption_updated_at = ?, checkout_adoption_synced_at = ?,
                 conflict_summary = ?, completed_at = ?
             WHERE id = ?",
        )
        .bind(optional_db_text(state.integration_workspace_id()))
        .bind(db_text(&convergence.source_head_commit_oid))
        .bind(db_text(&convergence.target_ref))
        .bind(db_text(convergence.strategy))
        .bind(db_text(state.status()))
        .bind(optional_db_text(state.input_target_commit_oid().cloned()))
        .bind(optional_db_text(state.prepared_commit_oid().cloned()))
        .bind(optional_db_text(state.final_target_commit_oid().cloned()))
        .bind(optional_db_text(state.checkout_adoption_state()))
        .bind(state.checkout_adoption_message())
        .bind(state.checkout_adoption_updated_at())
        .bind(state.checkout_adoption_synced_at())
        .bind(state.conflict_summary())
        .bind(state.completed_at())
        .bind(db_text(convergence.id))
        .execute(&mut *tx)
        .await
        .map_err(db_write_err)?;

        ensure_rows_affected(result)?;

        // 2. Update workspace (mark as stale) if present
        if let Some(workspace) = &mutation.workspace_update {
            let result = sqlx::query(
                "UPDATE workspaces
                 SET path = ?, target_ref = ?, workspace_ref = ?, base_commit_oid = ?,
                     head_commit_oid = ?, retention_policy = ?, status = ?,
                     current_job_id = ?, updated_at = ?
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
            .execute(&mut *tx)
            .await
            .map_err(db_write_err)?;

            ensure_rows_affected(result)?;
        }

        // 3. Update item (reset approval state)
        let item = &mutation.item;
        let result = sqlx::query(
            "UPDATE items
             SET approval_state = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(db_text(item.approval_state))
        .bind(item.updated_at)
        .bind(db_text(item.id))
        .execute(&mut *tx)
        .await
        .map_err(db_write_err)?;

        ensure_rows_affected(result)?;

        // 4. Append activity
        let activity = &mutation.activity;
        sqlx::query(
            "INSERT INTO activity (
                id, project_id, event_type, entity_type, entity_id, payload, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(db_text(activity.id))
        .bind(db_text(activity.project_id))
        .bind(db_text(activity.event_type))
        .bind(db_text(activity.subject.entity_type()))
        .bind(activity.subject.entity_id_string())
        .bind(serde_json::to_string(&activity.payload).map_err(json_err)?)
        .bind(activity.created_at)
        .execute(&mut *tx)
        .await
        .map_err(db_write_err)?;

        tx.commit().await.map_err(db_err)?;
        Ok(())
    }
}

impl InvalidatePreparedConvergenceRepository for Database {
    async fn apply_invalidate_prepared_convergence(
        &self,
        mutation: InvalidatePreparedConvergenceMutation,
    ) -> Result<(), RepositoryError> {
        Database::apply_invalidate_prepared_convergence(self, mutation).await
    }
}
