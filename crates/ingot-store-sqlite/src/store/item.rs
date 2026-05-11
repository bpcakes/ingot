use ingot_domain::ids::{ItemId, ProjectId};
use ingot_domain::item::{Escalation, Item, Lifecycle, Origin};
use ingot_domain::ports::{ItemRepository, RepositoryError};
use ingot_domain::revision::ItemRevision;
use sqlx::sqlite::SqliteRow;

use super::helpers::{
    db_err, db_text, db_write_err, ensure_rows_affected, json_err, optional_db_text, required_row,
    row_get, row_get_json,
};
use crate::db::Database;

type SqliteQuery<'a> = sqlx::query::Query<'a, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'a>>;

pub(super) fn lifecycle_state(lifecycle: &Lifecycle) -> &'static str {
    match lifecycle {
        Lifecycle::Open => "open",
        Lifecycle::Done { .. } => "done",
    }
}

pub(super) fn escalation_state(escalation: &Escalation) -> &'static str {
    match escalation {
        Escalation::None => "none",
        Escalation::OperatorRequired { .. } => "operator_required",
    }
}

pub(super) fn origin_kind(origin: &Origin) -> &'static str {
    match origin {
        Origin::Manual => "manual",
        Origin::PromotedFinding { .. } => "promoted_finding",
    }
}

impl Database {
    pub async fn list_items_by_project(
        &self,
        project_id: ProjectId,
    ) -> Result<Vec<Item>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT * FROM items WHERE project_id = ? ORDER BY sort_key ASC, created_at ASC",
        )
        .bind(db_text(project_id))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.iter().map(map_item).collect()
    }

    pub async fn get_item(&self, item_id: ItemId) -> Result<Item, RepositoryError> {
        let row = sqlx::query("SELECT * FROM items WHERE id = ?")
            .bind(db_text(item_id))
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;

        required_row(row, map_item)
    }

    pub async fn update_item(&self, item: &Item) -> Result<(), RepositoryError> {
        let result = sqlx::query(
            "UPDATE items
             SET classification = ?, workflow_version = ?, lifecycle_state = ?, parking_state = ?,
                 done_reason = ?, resolution_source = ?, approval_state = ?, escalation_state = ?,
                 escalation_reason = ?, current_revision_id = ?, origin_kind = ?, origin_finding_id = ?,
                 priority = ?, labels = ?, operator_notes = ?, updated_at = ?, closed_at = ?
             WHERE id = ?",
        )
        .bind(db_text(item.classification))
        .bind(db_text(item.workflow_version))
        .bind(lifecycle_state(&item.lifecycle))
        .bind(db_text(item.parking_state))
        .bind(optional_db_text(item.lifecycle.done_reason()))
        .bind(optional_db_text(item.lifecycle.resolution_source()))
        .bind(db_text(item.approval_state))
        .bind(escalation_state(&item.escalation))
        .bind(optional_db_text(item.escalation.reason()))
        .bind(db_text(item.current_revision_id))
        .bind(origin_kind(&item.origin))
        .bind(optional_db_text(item.origin.finding_id()))
        .bind(db_text(item.priority))
        .bind(serde_json::to_string(&item.labels).map_err(json_err)?)
        .bind(item.operator_notes.as_deref())
        .bind(item.updated_at)
        .bind(item.lifecycle.closed_at())
        .bind(db_text(item.id))
        .execute(&self.pool)
        .await
        .map_err(db_write_err)?;

        ensure_rows_affected(result)
    }

    pub async fn create_item_with_revision(
        &self,
        item: &Item,
        revision: &ItemRevision,
    ) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;

        insert_item_query(item)?
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        insert_revision_query(revision)?
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        tx.commit().await.map_err(db_err)?;
        Ok(())
    }

    pub async fn create_item(&self, item: &Item) -> Result<(), RepositoryError> {
        insert_item_query(item)?
            .execute(&self.pool)
            .await
            .map_err(db_write_err)?;

        Ok(())
    }
}

impl ItemRepository for Database {
    async fn list_by_project(&self, project_id: ProjectId) -> Result<Vec<Item>, RepositoryError> {
        self.list_items_by_project(project_id).await
    }
    async fn get(&self, id: ItemId) -> Result<Item, RepositoryError> {
        self.get_item(id).await
    }
    async fn create(&self, item: &Item) -> Result<(), RepositoryError> {
        self.create_item(item).await
    }
    async fn update(&self, item: &Item) -> Result<(), RepositoryError> {
        self.update_item(item).await
    }
    async fn create_with_revision(
        &self,
        item: &Item,
        revision: &ItemRevision,
    ) -> Result<(), RepositoryError> {
        self.create_item_with_revision(item, revision).await
    }
}

pub(crate) fn insert_revision_query<'a>(
    revision: &'a ItemRevision,
) -> Result<SqliteQuery<'a>, RepositoryError> {
    Ok(sqlx::query(
        "INSERT INTO item_revisions (
            id, item_id, revision_no, title, description, acceptance_criteria, target_ref,
            approval_policy, policy_snapshot, template_map_snapshot, seed_commit_oid,
            seed_target_commit_oid, supersedes_revision_id, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(db_text(revision.id))
    .bind(db_text(revision.item_id))
    .bind(revision.revision_no as i64)
    .bind(&revision.title)
    .bind(&revision.description)
    .bind(&revision.acceptance_criteria)
    .bind(db_text(&revision.target_ref))
    .bind(db_text(revision.approval_policy))
    .bind(serde_json::to_string(&revision.policy_snapshot).map_err(json_err)?)
    .bind(serde_json::to_string(&revision.template_map_snapshot).map_err(json_err)?)
    .bind(optional_db_text(revision.seed.seed_commit_oid().cloned()))
    .bind(db_text(revision.seed.seed_target_commit_oid()))
    .bind(optional_db_text(revision.supersedes_revision_id))
    .bind(revision.created_at))
}

pub(crate) fn insert_item_query<'a>(item: &'a Item) -> Result<SqliteQuery<'a>, RepositoryError> {
    Ok(sqlx::query(
        "INSERT INTO items (
            id, project_id, classification, workflow_version, lifecycle_state, parking_state,
            done_reason, resolution_source, approval_state, escalation_state, escalation_reason,
            current_revision_id, origin_kind, origin_finding_id, priority, labels, operator_notes,
            sort_key, created_at, updated_at, closed_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(db_text(item.id))
    .bind(db_text(item.project_id))
    .bind(db_text(item.classification))
    .bind(db_text(item.workflow_version))
    .bind(lifecycle_state(&item.lifecycle))
    .bind(db_text(item.parking_state))
    .bind(optional_db_text(item.lifecycle.done_reason()))
    .bind(optional_db_text(item.lifecycle.resolution_source()))
    .bind(db_text(item.approval_state))
    .bind(escalation_state(&item.escalation))
    .bind(optional_db_text(item.escalation.reason()))
    .bind(db_text(item.current_revision_id))
    .bind(origin_kind(&item.origin))
    .bind(optional_db_text(item.origin.finding_id()))
    .bind(db_text(item.priority))
    .bind(serde_json::to_string(&item.labels).map_err(json_err)?)
    .bind(item.operator_notes.as_deref())
    .bind(&item.sort_key)
    .bind(item.created_at)
    .bind(item.updated_at)
    .bind(item.lifecycle.closed_at()))
}

fn map_item(row: &SqliteRow) -> Result<Item, RepositoryError> {
    Ok(Item {
        id: row_get(row, "id")?,
        project_id: row_get(row, "project_id")?,
        classification: row_get(row, "classification")?,
        workflow_version: row_get(row, "workflow_version")?,
        lifecycle: parse_lifecycle(row)?,
        parking_state: row_get(row, "parking_state")?,
        approval_state: row_get(row, "approval_state")?,
        escalation: parse_escalation(row)?,
        current_revision_id: row_get(row, "current_revision_id")?,
        origin: parse_origin(row)?,
        priority: row_get(row, "priority")?,
        labels: row_get_json(row, "labels")?,
        operator_notes: row_get(row, "operator_notes")?,
        sort_key: row_get(row, "sort_key")?,
        created_at: row_get(row, "created_at")?,
        updated_at: row_get(row, "updated_at")?,
    })
}

fn parse_lifecycle(row: &SqliteRow) -> Result<Lifecycle, RepositoryError> {
    match row_get::<String>(row, "lifecycle_state")?.as_str() {
        "open" => Ok(Lifecycle::Open),
        "done" => Ok(Lifecycle::Done {
            reason: row_get(row, "done_reason")?,
            source: row_get(row, "resolution_source")?,
            closed_at: row_get(row, "closed_at")?,
        }),
        other => invalid_state("lifecycle_state", other),
    }
}

fn parse_escalation(row: &SqliteRow) -> Result<Escalation, RepositoryError> {
    match row_get::<String>(row, "escalation_state")?.as_str() {
        "none" => Ok(Escalation::None),
        "operator_required" => Ok(Escalation::OperatorRequired {
            reason: row_get(row, "escalation_reason")?,
        }),
        other => invalid_state("escalation_state", other),
    }
}

fn parse_origin(row: &SqliteRow) -> Result<Origin, RepositoryError> {
    match row_get::<String>(row, "origin_kind")?.as_str() {
        "manual" => Ok(Origin::Manual),
        "promoted_finding" => Ok(Origin::PromotedFinding {
            finding_id: row_get(row, "origin_finding_id")?,
        }),
        other => invalid_state("origin_kind", other),
    }
}

fn invalid_state<T>(field: &str, value: &str) -> Result<T, RepositoryError> {
    Err(RepositoryError::Database(
        format!("unknown {field}: {value}").into(),
    ))
}
