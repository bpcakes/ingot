use ingot_domain::activity::{Activity, ActivitySubject};
use ingot_domain::ids::ProjectId;
use ingot_domain::ports::{ActivityRepository, ConflictKind, RepositoryError};
use sqlx::sqlite::SqliteRow;

use super::helpers::{db_err, db_write_err, json_err, row_get, row_get_json};
use crate::db::Database;

impl Database {
    pub async fn append_activity(&self, activity: &Activity) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO activity (
                id, project_id, event_type, entity_type, entity_id, payload, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(activity.id)
        .bind(activity.project_id)
        .bind(activity.event_type)
        .bind(activity.subject.entity_type())
        .bind(activity.subject.entity_id_string())
        .bind(serde_json::to_string(&activity.payload).map_err(json_err)?)
        .bind(activity.created_at)
        .execute(&self.pool)
        .await
        .map_err(db_write_err)?;

        Ok(())
    }

    pub async fn list_activity_by_project(
        &self,
        project_id: ProjectId,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Activity>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, project_id, event_type, entity_type, entity_id, payload, created_at
             FROM activity
             WHERE project_id = ?
             ORDER BY created_at DESC
             LIMIT ? OFFSET ?",
        )
        .bind(project_id)
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.iter().map(map_activity).collect()
    }
}

impl ActivityRepository for Database {
    async fn append(&self, activity: &Activity) -> Result<(), RepositoryError> {
        self.append_activity(activity).await
    }
    async fn list_by_project(
        &self,
        project_id: ProjectId,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Activity>, RepositoryError> {
        self.list_activity_by_project(project_id, limit, offset)
            .await
    }
}

fn map_activity(row: &SqliteRow) -> Result<Activity, RepositoryError> {
    let entity_type = row_get(row, "entity_type")?;
    let entity_id: String = row_get(row, "entity_id")?;
    let subject = ActivitySubject::from_parts(entity_type, &entity_id).map_err(|e| {
        RepositoryError::Conflict(ConflictKind::Other(format!(
            "invalid activity subject: {e}"
        )))
    })?;
    Ok(Activity {
        id: row_get(row, "id")?,
        project_id: row_get(row, "project_id")?,
        event_type: row_get(row, "event_type")?,
        subject,
        payload: row_get_json(row, "payload")?,
        created_at: row_get(row, "created_at")?,
    })
}
