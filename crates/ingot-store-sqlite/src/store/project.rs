use std::path::PathBuf;

use ingot_domain::ids::ProjectId;
use ingot_domain::ports::{ProjectRepository, RepositoryError};
use ingot_domain::project::{AgentRouting, AutoTriagePolicy, Project};
use sqlx::sqlite::SqliteRow;

use super::helpers::{
    db_err, db_write_err, ensure_rows_affected, required_row, row_get, row_get_optional_json,
    serialize_optional_json,
};
use crate::db::Database;

impl Database {
    pub async fn list_projects(&self) -> Result<Vec<Project>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, name, path, default_branch, color, execution_mode, agent_routing, auto_triage_policy, created_at, updated_at
             FROM projects
             ORDER BY name ASC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.iter().map(map_project).collect()
    }

    pub async fn get_project(&self, project_id: ProjectId) -> Result<Project, RepositoryError> {
        let row = sqlx::query(
            "SELECT id, name, path, default_branch, color, execution_mode, agent_routing, auto_triage_policy, created_at, updated_at
             FROM projects
             WHERE id = ?",
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;

        required_row(row, map_project)
    }

    pub async fn create_project(&self, project: &Project) -> Result<(), RepositoryError> {
        let agent_routing_json = serialize_optional_json(project.agent_routing.as_ref())?;
        let auto_triage_policy_json = serialize_optional_json(project.auto_triage_policy.as_ref())?;
        sqlx::query(
            "INSERT INTO projects (id, name, path, default_branch, color, execution_mode, agent_routing, auto_triage_policy, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(project.id)
        .bind(&project.name)
        .bind(project.path.to_string_lossy().as_ref())
        .bind(&project.default_branch)
        .bind(&project.color)
        .bind(project.execution_mode)
        .bind(&agent_routing_json)
        .bind(&auto_triage_policy_json)
        .bind(project.created_at)
        .bind(project.updated_at)
        .execute(&self.pool)
        .await
        .map_err(db_write_err)?;

        Ok(())
    }

    pub async fn update_project(&self, project: &Project) -> Result<(), RepositoryError> {
        let agent_routing_json = serialize_optional_json(project.agent_routing.as_ref())?;
        let auto_triage_policy_json = serialize_optional_json(project.auto_triage_policy.as_ref())?;
        let result = sqlx::query(
            "UPDATE projects
             SET name = ?, path = ?, default_branch = ?, color = ?, execution_mode = ?, agent_routing = ?, auto_triage_policy = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(&project.name)
        .bind(project.path.to_string_lossy().as_ref())
        .bind(&project.default_branch)
        .bind(&project.color)
        .bind(project.execution_mode)
        .bind(&agent_routing_json)
        .bind(&auto_triage_policy_json)
        .bind(project.updated_at)
        .bind(project.id)
        .execute(&self.pool)
        .await
        .map_err(db_write_err)?;

        ensure_rows_affected(result)
    }

    pub async fn delete_project(&self, project_id: ProjectId) -> Result<(), RepositoryError> {
        let result = sqlx::query("DELETE FROM projects WHERE id = ?")
            .bind(project_id)
            .execute(&self.pool)
            .await
            .map_err(db_write_err)?;

        ensure_rows_affected(result)
    }
}

impl ProjectRepository for Database {
    async fn list(&self) -> Result<Vec<Project>, RepositoryError> {
        self.list_projects().await
    }
    async fn get(&self, id: ProjectId) -> Result<Project, RepositoryError> {
        self.get_project(id).await
    }
    async fn create(&self, project: &Project) -> Result<(), RepositoryError> {
        self.create_project(project).await
    }
    async fn update(&self, project: &Project) -> Result<(), RepositoryError> {
        self.update_project(project).await
    }
    async fn delete(&self, id: ProjectId) -> Result<(), RepositoryError> {
        self.delete_project(id).await
    }
}

fn map_project(row: &SqliteRow) -> Result<Project, RepositoryError> {
    let agent_routing: Option<AgentRouting> = row_get_optional_json(row, "agent_routing")?;
    let auto_triage_policy: Option<AutoTriagePolicy> =
        row_get_optional_json(row, "auto_triage_policy")?;

    Ok(Project {
        id: row_get(row, "id")?,
        name: row_get(row, "name")?,
        path: PathBuf::from(row_get::<String>(row, "path")?),
        default_branch: row_get(row, "default_branch")?,
        color: row_get(row, "color")?,
        execution_mode: row_get(row, "execution_mode")?,
        agent_routing,
        auto_triage_policy,
        created_at: row_get(row, "created_at")?,
        updated_at: row_get(row, "updated_at")?,
    })
}
