use ingot_domain::commit_oid::CommitOid;
use ingot_domain::convergence::{
    CheckoutAdoptionState, Convergence, ConvergenceState, ConvergenceStateParts, ConvergenceStatus,
};
use ingot_domain::ids::{ConvergenceId, ItemId, ItemRevisionId};
use ingot_domain::ports::{ConvergenceRepository, RepositoryError};
use sqlx::sqlite::SqliteRow;

use super::helpers::{
    db_err, db_write_err, ensure_rows_affected, map_optional_row, required_row, row_get,
};
use crate::db::Database;

impl Database {
    pub async fn list_convergences_by_item(
        &self,
        item_id: ItemId,
    ) -> Result<Vec<Convergence>, RepositoryError> {
        let rows =
            sqlx::query("SELECT * FROM convergences WHERE item_id = ? ORDER BY created_at DESC")
                .bind(item_id)
                .fetch_all(&self.pool)
                .await
                .map_err(db_err)?;

        rows.iter().map(map_convergence).collect()
    }

    pub async fn get_convergence(
        &self,
        convergence_id: ConvergenceId,
    ) -> Result<Convergence, RepositoryError> {
        let row = sqlx::query("SELECT * FROM convergences WHERE id = ?")
            .bind(convergence_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;

        required_row(row, map_convergence)
    }

    pub async fn list_active_convergences(&self) -> Result<Vec<Convergence>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT *
             FROM convergences
             WHERE status IN ('queued', 'running')
             ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.iter().map(map_convergence).collect()
    }

    pub async fn create_convergence(
        &self,
        convergence: &Convergence,
    ) -> Result<(), RepositoryError> {
        let state = &convergence.state;

        sqlx::query(
            "INSERT INTO convergences (
                id, project_id, item_id, item_revision_id, source_workspace_id, integration_workspace_id,
                source_head_commit_oid, target_ref, strategy, status, input_target_commit_oid,
                prepared_commit_oid, final_target_commit_oid, checkout_adoption_state,
                checkout_adoption_message, checkout_adoption_updated_at, checkout_adoption_synced_at,
                conflict_summary, created_at, completed_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(convergence.id)
        .bind(convergence.project_id)
        .bind(convergence.item_id)
        .bind(convergence.item_revision_id)
        .bind(convergence.source_workspace_id)
        .bind(state.integration_workspace_id())
        .bind(convergence.source_head_commit_oid.clone())
        .bind(&convergence.target_ref)
        .bind(convergence.strategy)
        .bind(state.status())
        .bind(state.input_target_commit_oid().cloned())
        .bind(state.prepared_commit_oid().cloned())
        .bind(state.final_target_commit_oid().cloned())
        .bind(state.checkout_adoption_state())
        .bind(state.checkout_adoption_message())
        .bind(state.checkout_adoption_updated_at())
        .bind(state.checkout_adoption_synced_at())
        .bind(state.conflict_summary())
        .bind(convergence.created_at)
        .bind(state.completed_at())
        .execute(&self.pool)
        .await
        .map_err(db_write_err)?;

        Ok(())
    }

    pub async fn update_convergence(
        &self,
        convergence: &Convergence,
    ) -> Result<(), RepositoryError> {
        let state = &convergence.state;

        let result = sqlx::query(
            "UPDATE convergences
             SET integration_workspace_id = ?, source_head_commit_oid = ?, target_ref = ?, strategy = ?,
                 status = ?, input_target_commit_oid = ?, prepared_commit_oid = ?, final_target_commit_oid = ?,
                 checkout_adoption_state = ?, checkout_adoption_message = ?,
                 checkout_adoption_updated_at = ?, checkout_adoption_synced_at = ?,
                 conflict_summary = ?, completed_at = ?
             WHERE id = ?",
        )
        .bind(state.integration_workspace_id())
        .bind(convergence.source_head_commit_oid.clone())
        .bind(&convergence.target_ref)
        .bind(convergence.strategy)
        .bind(state.status())
        .bind(state.input_target_commit_oid().cloned())
        .bind(state.prepared_commit_oid().cloned())
        .bind(state.final_target_commit_oid().cloned())
        .bind(state.checkout_adoption_state())
        .bind(state.checkout_adoption_message())
        .bind(state.checkout_adoption_updated_at())
        .bind(state.checkout_adoption_synced_at())
        .bind(state.conflict_summary())
        .bind(state.completed_at())
        .bind(convergence.id)
        .execute(&self.pool)
        .await
        .map_err(db_write_err)?;

        ensure_rows_affected(result)
    }

    pub async fn list_convergences_by_revision(
        &self,
        revision_id: ItemRevisionId,
    ) -> Result<Vec<Convergence>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT * FROM convergences WHERE item_revision_id = ? ORDER BY created_at DESC",
        )
        .bind(revision_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.iter().map(map_convergence).collect()
    }

    pub async fn find_active_convergence_for_revision(
        &self,
        revision_id: ItemRevisionId,
    ) -> Result<Option<Convergence>, RepositoryError> {
        let row = sqlx::query(
            "SELECT *
             FROM convergences
             WHERE item_revision_id = ?
               AND status IN ('queued', 'running')
             ORDER BY created_at DESC
             LIMIT 1",
        )
        .bind(revision_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;

        map_optional_row(row, map_convergence)
    }

    pub async fn find_prepared_convergence_for_revision(
        &self,
        revision_id: ItemRevisionId,
    ) -> Result<Option<Convergence>, RepositoryError> {
        let row = sqlx::query(
            "SELECT *
             FROM convergences
             WHERE item_revision_id = ?
               AND status = 'prepared'
             ORDER BY created_at DESC
             LIMIT 1",
        )
        .bind(revision_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;

        map_optional_row(row, map_convergence)
    }
}

impl ConvergenceRepository for Database {
    async fn list_by_revision(
        &self,
        revision_id: ItemRevisionId,
    ) -> Result<Vec<Convergence>, RepositoryError> {
        self.list_convergences_by_revision(revision_id).await
    }
    async fn get(&self, id: ConvergenceId) -> Result<Convergence, RepositoryError> {
        self.get_convergence(id).await
    }
    async fn create(&self, convergence: &Convergence) -> Result<(), RepositoryError> {
        self.create_convergence(convergence).await
    }
    async fn update(&self, convergence: &Convergence) -> Result<(), RepositoryError> {
        self.update_convergence(convergence).await
    }
    async fn find_active_for_revision(
        &self,
        revision_id: ItemRevisionId,
    ) -> Result<Option<Convergence>, RepositoryError> {
        self.find_active_convergence_for_revision(revision_id).await
    }
    async fn find_prepared_for_revision(
        &self,
        revision_id: ItemRevisionId,
    ) -> Result<Option<Convergence>, RepositoryError> {
        self.find_prepared_convergence_for_revision(revision_id)
            .await
    }
    async fn list_by_item(&self, item_id: ItemId) -> Result<Vec<Convergence>, RepositoryError> {
        self.list_convergences_by_item(item_id).await
    }
    async fn list_active(&self) -> Result<Vec<Convergence>, RepositoryError> {
        self.list_active_convergences().await
    }
}

fn map_convergence(row: &SqliteRow) -> Result<Convergence, RepositoryError> {
    let status: ConvergenceStatus = row_get(row, "status")?;

    let integration_workspace_id: Option<ingot_domain::ids::WorkspaceId> =
        row_get(row, "integration_workspace_id")?;
    let input_target_commit_oid: Option<CommitOid> = row_get(row, "input_target_commit_oid")?;
    let prepared_commit_oid: Option<CommitOid> = row_get(row, "prepared_commit_oid")?;
    let final_target_commit_oid: Option<CommitOid> = row_get(row, "final_target_commit_oid")?;
    let checkout_adoption_state: Option<CheckoutAdoptionState> =
        row_get(row, "checkout_adoption_state")?;
    let checkout_adoption_message: Option<String> = row_get(row, "checkout_adoption_message")?;
    let checkout_adoption_updated_at: Option<chrono::DateTime<chrono::Utc>> =
        row_get(row, "checkout_adoption_updated_at")?;
    let checkout_adoption_synced_at: Option<chrono::DateTime<chrono::Utc>> =
        row_get(row, "checkout_adoption_synced_at")?;
    let conflict_summary: Option<String> = row_get(row, "conflict_summary")?;
    let completed_at: Option<chrono::DateTime<chrono::Utc>> = row_get(row, "completed_at")?;

    let state = ConvergenceState::from_parts(
        status,
        ConvergenceStateParts {
            integration_workspace_id,
            input_target_commit_oid,
            prepared_commit_oid,
            final_target_commit_oid,
            checkout_adoption_state,
            checkout_adoption_message,
            checkout_adoption_updated_at,
            checkout_adoption_synced_at,
            conflict_summary,
            completed_at,
        },
    )
    .map_err(|error| RepositoryError::Database(error.into()))?;

    Ok(Convergence {
        id: row_get(row, "id")?,
        project_id: row_get(row, "project_id")?,
        item_id: row_get(row, "item_id")?,
        item_revision_id: row_get(row, "item_revision_id")?,
        source_workspace_id: row_get(row, "source_workspace_id")?,
        source_head_commit_oid: row_get(row, "source_head_commit_oid")?,
        target_ref: row_get(row, "target_ref")?,
        strategy: row_get(row, "strategy")?,
        target_head_valid: None,
        created_at: row_get(row, "created_at")?,
        state,
    })
}
