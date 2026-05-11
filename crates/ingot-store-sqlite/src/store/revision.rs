use ingot_domain::commit_oid::CommitOid;
use ingot_domain::ids::{ItemId, ItemRevisionId};
use ingot_domain::ports::{
    ConflictKind, RepositoryError, RevisionContextRepository, RevisionRepository,
};
use ingot_domain::revision::{AuthoringBaseSeed, ItemRevision};
use ingot_domain::revision_context::RevisionContext;
use sqlx::sqlite::SqliteRow;

use super::item::insert_revision_query;

use super::helpers::{
    db_err, db_text, db_write_err, json_err, map_optional_row, optional_db_text, required_row,
    row_get, row_get_json,
};
use crate::db::Database;

impl Database {
    pub async fn list_revisions_by_item(
        &self,
        item_id: ItemId,
    ) -> Result<Vec<ItemRevision>, RepositoryError> {
        let rows =
            sqlx::query("SELECT * FROM item_revisions WHERE item_id = ? ORDER BY revision_no DESC")
                .bind(db_text(item_id))
                .fetch_all(&self.pool)
                .await
                .map_err(db_err)?;

        rows.iter().map(map_revision).collect()
    }

    pub async fn get_revision(
        &self,
        revision_id: ItemRevisionId,
    ) -> Result<ItemRevision, RepositoryError> {
        let row = sqlx::query("SELECT * FROM item_revisions WHERE id = ?")
            .bind(db_text(revision_id))
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;

        required_row(row, map_revision)
    }

    pub async fn create_revision(&self, revision: &ItemRevision) -> Result<(), RepositoryError> {
        insert_revision_query(revision)?
            .execute(&self.pool)
            .await
            .map_err(db_write_err)?;

        Ok(())
    }

    pub async fn get_revision_context(
        &self,
        revision_id: ItemRevisionId,
    ) -> Result<Option<RevisionContext>, RepositoryError> {
        let row = sqlx::query("SELECT * FROM revision_contexts WHERE item_revision_id = ?")
            .bind(db_text(revision_id))
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;

        map_optional_row(row, map_revision_context)
    }

    pub async fn upsert_revision_context(
        &self,
        context: &RevisionContext,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO revision_contexts (
                item_revision_id, schema_version, payload, updated_from_job_id, updated_at
             ) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(item_revision_id) DO UPDATE SET
                schema_version = excluded.schema_version,
                payload = excluded.payload,
                updated_from_job_id = excluded.updated_from_job_id,
                updated_at = excluded.updated_at",
        )
        .bind(db_text(context.item_revision_id))
        .bind(&context.schema_version)
        .bind(serde_json::to_string(&context.payload).map_err(json_err)?)
        .bind(optional_db_text(context.updated_from_job_id))
        .bind(context.updated_at)
        .execute(&self.pool)
        .await
        .map_err(db_write_err)?;

        Ok(())
    }
}

impl RevisionRepository for Database {
    async fn list_by_item(&self, item_id: ItemId) -> Result<Vec<ItemRevision>, RepositoryError> {
        self.list_revisions_by_item(item_id).await
    }
    async fn get(&self, id: ItemRevisionId) -> Result<ItemRevision, RepositoryError> {
        self.get_revision(id).await
    }
    async fn create(&self, revision: &ItemRevision) -> Result<(), RepositoryError> {
        self.create_revision(revision).await
    }
}

impl RevisionContextRepository for Database {
    async fn get(
        &self,
        revision_id: ItemRevisionId,
    ) -> Result<Option<RevisionContext>, RepositoryError> {
        self.get_revision_context(revision_id).await
    }
    async fn upsert(&self, context: &RevisionContext) -> Result<(), RepositoryError> {
        self.upsert_revision_context(context).await
    }
}

fn map_revision(row: &SqliteRow) -> Result<ItemRevision, RepositoryError> {
    let seed_commit_oid: Option<CommitOid> = row_get(row, "seed_commit_oid")?;
    let seed_target_commit_oid: CommitOid =
        row_get::<Option<CommitOid>>(row, "seed_target_commit_oid")?.ok_or_else(|| {
            RepositoryError::Conflict(ConflictKind::Other(
                "seed_target_commit_oid must not be NULL".into(),
            ))
        })?;
    let seed = AuthoringBaseSeed::from_parts(seed_commit_oid, seed_target_commit_oid);

    Ok(ItemRevision {
        id: row_get(row, "id")?,
        item_id: row_get(row, "item_id")?,
        revision_no: row_get::<i64>(row, "revision_no")? as u32,
        title: row_get(row, "title")?,
        description: row_get(row, "description")?,
        acceptance_criteria: row_get(row, "acceptance_criteria")?,
        target_ref: row_get(row, "target_ref")?,
        approval_policy: row_get(row, "approval_policy")?,
        policy_snapshot: row_get_json(row, "policy_snapshot")?,
        template_map_snapshot: row_get_json(row, "template_map_snapshot")?,
        seed,
        supersedes_revision_id: row_get(row, "supersedes_revision_id")?,
        created_at: row_get(row, "created_at")?,
    })
}

fn map_revision_context(row: &SqliteRow) -> Result<RevisionContext, RepositoryError> {
    Ok(RevisionContext {
        item_revision_id: row_get(row, "item_revision_id")?,
        schema_version: row_get(row, "schema_version")?,
        payload: row_get_json(row, "payload")?,
        updated_from_job_id: row_get(row, "updated_from_job_id")?,
        updated_at: row_get(row, "updated_at")?,
    })
}
