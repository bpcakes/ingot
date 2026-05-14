use ingot_domain::git_operation::GitOperationWire;
use ingot_domain::ports::{
    PrepareConvergenceFailureMutation, PrepareConvergenceFailureRepository, RepositoryError,
};

use super::helpers::{
    db_err, db_text, db_write_err, ensure_rows_affected, json_err, optional_db_text,
};
use super::item::escalation_state;
use crate::db::Database;

impl Database {
    pub async fn apply_prepare_convergence_failure(
        &self,
        mutation: PrepareConvergenceFailureMutation,
    ) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;

        let workspace = &mutation.workspace;
        // The runtime holds the project mutation lock while preparing convergence. Workspace
        // status can vary by failure point; allow error for retry/recovery attempts, but do not
        // overwrite terminal/operator-owned states: retained_for_debug, removing, or abandoned.
        let workspace_result = sqlx::query(
            "UPDATE workspaces
             SET path = ?, target_ref = ?, workspace_ref = ?, base_commit_oid = ?,
                 head_commit_oid = ?, retention_policy = ?, status = ?,
                 current_job_id = ?, updated_at = ?
             WHERE id = ?
               AND status IN ('provisioning', 'ready', 'busy', 'stale', 'error')",
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
        ensure_rows_affected(workspace_result)?;

        let convergence = &mutation.convergence;
        let state = &convergence.state;
        let convergence_result = sqlx::query(
            "UPDATE convergences
             SET integration_workspace_id = ?, source_head_commit_oid = ?, target_ref = ?,
                 strategy = ?, status = ?, input_target_commit_oid = ?,
                 prepared_commit_oid = ?, final_target_commit_oid = ?,
                 checkout_adoption_state = ?, checkout_adoption_message = ?,
                 checkout_adoption_updated_at = ?, checkout_adoption_synced_at = ?,
                 conflict_summary = ?, completed_at = ?
             WHERE id = ?
               AND status = 'running'",
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
        ensure_rows_affected(convergence_result)?;

        // This transition only changes approval/escalation state; preserve unrelated item fields
        // and refuse to clobber a stale, closed, or differently escalated item. Re-applying the
        // same escalation reason is allowed so retry/recovery paths can converge idempotently.
        let item = &mutation.item;
        let escalation_reason = item.escalation.reason();
        let item_result = sqlx::query(
            "UPDATE items
             SET approval_state = ?,
                 escalation_state = ?,
                 escalation_reason = ?,
                 updated_at = ?
             WHERE id = ?
               AND current_revision_id = ?
               AND lifecycle_state = 'open'
               AND (
                    escalation_state = 'none'
                    OR (escalation_state = 'operator_required' AND escalation_reason = ?)
               )",
        )
        .bind(db_text(item.approval_state))
        .bind(escalation_state(&item.escalation))
        .bind(optional_db_text(escalation_reason))
        .bind(item.updated_at)
        .bind(db_text(item.id))
        .bind(db_text(mutation.queue_entry.item_revision_id))
        .bind(optional_db_text(escalation_reason))
        .execute(&mut *tx)
        .await
        .map_err(db_write_err)?;
        ensure_rows_affected(item_result)?;

        let queue_entry = &mutation.queue_entry;
        let queue_result = sqlx::query(
            "UPDATE convergence_queue_entries
             SET status = ?, head_acquired_at = ?, updated_at = ?, released_at = ?
             WHERE id = ? AND status = 'head'",
        )
        .bind(db_text(queue_entry.status))
        .bind(queue_entry.head_acquired_at)
        .bind(queue_entry.updated_at)
        .bind(queue_entry.released_at)
        .bind(db_text(queue_entry.id))
        .execute(&mut *tx)
        .await
        .map_err(db_write_err)?;
        ensure_rows_affected(queue_result)?;

        let git_operation = GitOperationWire::from(&mutation.git_operation);
        // Prepare operations remain planned until they either produce a commit or fail; guard
        // this terminal update so completed/reconciled operations cannot be overwritten.
        let git_operation_result = sqlx::query(
            "UPDATE git_operations
             SET workspace_id = ?, ref_name = ?, expected_old_oid = ?, new_oid = ?, commit_oid = ?,
                 status = ?, metadata = ?, completed_at = ?
             WHERE id = ? AND status = 'planned'",
        )
        .bind(optional_db_text(git_operation.workspace_id))
        .bind(optional_db_text(git_operation.ref_name.clone()))
        .bind(optional_db_text(git_operation.expected_old_oid.clone()))
        .bind(optional_db_text(git_operation.new_oid.clone()))
        .bind(optional_db_text(git_operation.commit_oid.clone()))
        .bind(db_text(git_operation.status))
        .bind(
            git_operation
                .metadata
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(json_err)?,
        )
        .bind(git_operation.completed_at)
        .bind(db_text(git_operation.id))
        .execute(&mut *tx)
        .await
        .map_err(db_write_err)?;
        ensure_rows_affected(git_operation_result)?;

        for activity in &mutation.activities {
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
        }

        tx.commit().await.map_err(db_err)?;
        Ok(())
    }
}

impl PrepareConvergenceFailureRepository for Database {
    async fn apply_prepare_convergence_failure(
        &self,
        mutation: PrepareConvergenceFailureMutation,
    ) -> Result<(), RepositoryError> {
        Database::apply_prepare_convergence_failure(self, mutation).await
    }
}
