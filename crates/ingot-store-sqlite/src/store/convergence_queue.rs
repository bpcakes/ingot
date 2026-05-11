use ingot_domain::convergence_queue::ConvergenceQueueEntry;
use ingot_domain::git_ref::GitRef;
use ingot_domain::ids::{ConvergenceQueueEntryId, ItemId, ItemRevisionId, ProjectId};
use ingot_domain::ports::{ConvergenceQueueRepository, RepositoryError};
use sqlx::sqlite::SqliteRow;

use super::helpers::{
    db_err, db_text, db_write_err, ensure_rows_affected, map_optional_row, required_row, row_get,
};
use crate::db::Database;

impl Database {
    pub async fn list_queue_entries_by_item(
        &self,
        item_id: ItemId,
    ) -> Result<Vec<ConvergenceQueueEntry>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT *
             FROM convergence_queue_entries
             WHERE item_id = ?
             ORDER BY created_at ASC, id ASC",
        )
        .bind(db_text(item_id))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.iter().map(map_convergence_queue_entry).collect()
    }

    pub async fn get_queue_entry(
        &self,
        queue_entry_id: ConvergenceQueueEntryId,
    ) -> Result<ConvergenceQueueEntry, RepositoryError> {
        let row = sqlx::query("SELECT * FROM convergence_queue_entries WHERE id = ?")
            .bind(db_text(queue_entry_id))
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;

        required_row(row, map_convergence_queue_entry)
    }

    pub async fn find_active_queue_entry_for_revision(
        &self,
        revision_id: ItemRevisionId,
    ) -> Result<Option<ConvergenceQueueEntry>, RepositoryError> {
        let row = sqlx::query(
            "SELECT *
             FROM convergence_queue_entries
             WHERE item_revision_id = ?
               AND status IN ('queued', 'head')
             ORDER BY created_at ASC, id ASC
             LIMIT 1",
        )
        .bind(db_text(revision_id))
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;

        map_optional_row(row, map_convergence_queue_entry)
    }

    pub async fn find_queue_head(
        &self,
        project_id: ProjectId,
        target_ref: &GitRef,
    ) -> Result<Option<ConvergenceQueueEntry>, RepositoryError> {
        let row = sqlx::query(
            "SELECT *
             FROM convergence_queue_entries
             WHERE project_id = ?
               AND target_ref = ?
               AND status = 'head'
             LIMIT 1",
        )
        .bind(db_text(project_id))
        .bind(db_text(target_ref))
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;

        map_optional_row(row, map_convergence_queue_entry)
    }

    pub async fn find_next_queued_entry(
        &self,
        project_id: ProjectId,
        target_ref: &GitRef,
    ) -> Result<Option<ConvergenceQueueEntry>, RepositoryError> {
        let row = sqlx::query(
            "SELECT *
             FROM convergence_queue_entries
             WHERE project_id = ?
               AND target_ref = ?
               AND status = 'queued'
             ORDER BY created_at ASC, id ASC
             LIMIT 1",
        )
        .bind(db_text(project_id))
        .bind(db_text(target_ref))
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;

        map_optional_row(row, map_convergence_queue_entry)
    }

    pub async fn list_active_queue_entries_for_lane(
        &self,
        project_id: ProjectId,
        target_ref: &GitRef,
    ) -> Result<Vec<ConvergenceQueueEntry>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT *
             FROM convergence_queue_entries
             WHERE project_id = ?
               AND target_ref = ?
               AND status IN ('queued', 'head')
             ORDER BY created_at ASC, id ASC",
        )
        .bind(db_text(project_id))
        .bind(db_text(target_ref))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.iter().map(map_convergence_queue_entry).collect()
    }

    pub async fn list_active_queue_entries_by_project(
        &self,
        project_id: ProjectId,
    ) -> Result<Vec<ConvergenceQueueEntry>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT *
             FROM convergence_queue_entries
             WHERE project_id = ?
               AND status IN ('queued', 'head')
             ORDER BY target_ref ASC, created_at ASC, id ASC",
        )
        .bind(db_text(project_id))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.iter().map(map_convergence_queue_entry).collect()
    }

    pub async fn create_queue_entry(
        &self,
        queue_entry: &ConvergenceQueueEntry,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO convergence_queue_entries (
                id, project_id, item_id, item_revision_id, target_ref, status, head_acquired_at,
                created_at, updated_at, released_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(db_text(queue_entry.id))
        .bind(db_text(queue_entry.project_id))
        .bind(db_text(queue_entry.item_id))
        .bind(db_text(queue_entry.item_revision_id))
        .bind(db_text(&queue_entry.target_ref))
        .bind(db_text(queue_entry.status))
        .bind(queue_entry.head_acquired_at)
        .bind(queue_entry.created_at)
        .bind(queue_entry.updated_at)
        .bind(queue_entry.released_at)
        .execute(&self.pool)
        .await
        .map_err(db_write_err)?;

        Ok(())
    }

    pub async fn update_queue_entry(
        &self,
        queue_entry: &ConvergenceQueueEntry,
    ) -> Result<(), RepositoryError> {
        let result = sqlx::query(
            "UPDATE convergence_queue_entries
             SET status = ?, head_acquired_at = ?, updated_at = ?, released_at = ?
             WHERE id = ?",
        )
        .bind(db_text(queue_entry.status))
        .bind(queue_entry.head_acquired_at)
        .bind(queue_entry.updated_at)
        .bind(queue_entry.released_at)
        .bind(db_text(queue_entry.id))
        .execute(&self.pool)
        .await
        .map_err(db_write_err)?;

        ensure_rows_affected(result)
    }
}

impl ConvergenceQueueRepository for Database {
    async fn list_by_item(
        &self,
        item_id: ItemId,
    ) -> Result<Vec<ConvergenceQueueEntry>, RepositoryError> {
        self.list_queue_entries_by_item(item_id).await
    }
    async fn get(
        &self,
        id: ConvergenceQueueEntryId,
    ) -> Result<ConvergenceQueueEntry, RepositoryError> {
        self.get_queue_entry(id).await
    }
    async fn find_active_for_revision(
        &self,
        revision_id: ItemRevisionId,
    ) -> Result<Option<ConvergenceQueueEntry>, RepositoryError> {
        self.find_active_queue_entry_for_revision(revision_id).await
    }
    async fn find_head(
        &self,
        project_id: ProjectId,
        target_ref: &GitRef,
    ) -> Result<Option<ConvergenceQueueEntry>, RepositoryError> {
        self.find_queue_head(project_id, target_ref).await
    }
    async fn find_next_queued(
        &self,
        project_id: ProjectId,
        target_ref: &GitRef,
    ) -> Result<Option<ConvergenceQueueEntry>, RepositoryError> {
        self.find_next_queued_entry(project_id, target_ref).await
    }
    async fn list_active_by_project(
        &self,
        project_id: ProjectId,
    ) -> Result<Vec<ConvergenceQueueEntry>, RepositoryError> {
        self.list_active_queue_entries_by_project(project_id).await
    }
    async fn list_active_for_lane(
        &self,
        project_id: ProjectId,
        target_ref: &GitRef,
    ) -> Result<Vec<ConvergenceQueueEntry>, RepositoryError> {
        self.list_active_queue_entries_for_lane(project_id, target_ref)
            .await
    }
    async fn create(&self, entry: &ConvergenceQueueEntry) -> Result<(), RepositoryError> {
        self.create_queue_entry(entry).await
    }
    async fn update(&self, entry: &ConvergenceQueueEntry) -> Result<(), RepositoryError> {
        self.update_queue_entry(entry).await
    }
}

fn map_convergence_queue_entry(row: &SqliteRow) -> Result<ConvergenceQueueEntry, RepositoryError> {
    Ok(ConvergenceQueueEntry {
        id: row_get(row, "id")?,
        project_id: row_get(row, "project_id")?,
        item_id: row_get(row, "item_id")?,
        item_revision_id: row_get(row, "item_revision_id")?,
        target_ref: row_get(row, "target_ref")?,
        status: row_get(row, "status")?,
        head_acquired_at: row_get(row, "head_acquired_at")?,
        created_at: row_get(row, "created_at")?,
        updated_at: row_get(row, "updated_at")?,
        released_at: row_get(row, "released_at")?,
    })
}
