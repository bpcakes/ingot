use std::path::PathBuf;

use ingot_domain::agent::Agent;
use ingot_domain::ids::AgentId;
use ingot_domain::ports::{AgentRepository, RepositoryError};
use sqlx::sqlite::SqliteRow;

use super::helpers::{
    db_err, db_write_err, ensure_rows_affected, required_row, row_get, row_get_json, serialize_json,
};
use crate::db::Database;

impl Database {
    pub async fn list_agents(&self) -> Result<Vec<Agent>, RepositoryError> {
        let rows = sqlx::query(
            "SELECT id, slug, name, adapter_kind, provider, model, cli_path, capabilities,
                    health_check, status
             FROM agents
             ORDER BY slug ASC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;

        rows.iter().map(map_agent).collect()
    }

    pub async fn get_agent(&self, agent_id: AgentId) -> Result<Agent, RepositoryError> {
        let row = sqlx::query(
            "SELECT id, slug, name, adapter_kind, provider, model, cli_path, capabilities,
                    health_check, status
             FROM agents
             WHERE id = ?",
        )
        .bind(agent_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;

        required_row(row, map_agent)
    }

    pub async fn create_agent(&self, agent: &Agent) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO agents (
                id, slug, name, adapter_kind, provider, model, cli_path, capabilities,
                health_check, status
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(agent.id)
        .bind(&agent.slug)
        .bind(&agent.name)
        .bind(agent.adapter_kind)
        .bind(agent.provider)
        .bind(&agent.model)
        .bind(agent.cli_path.to_string_lossy().as_ref())
        .bind(serialize_json(&agent.capabilities)?)
        .bind(agent.health_check.as_deref())
        .bind(agent.status)
        .execute(&self.pool)
        .await
        .map_err(db_write_err)?;

        Ok(())
    }

    pub async fn update_agent(&self, agent: &Agent) -> Result<(), RepositoryError> {
        let result = sqlx::query(
            "UPDATE agents
             SET slug = ?, name = ?, adapter_kind = ?, provider = ?, model = ?, cli_path = ?,
                 capabilities = ?, health_check = ?, status = ?
             WHERE id = ?",
        )
        .bind(&agent.slug)
        .bind(&agent.name)
        .bind(agent.adapter_kind)
        .bind(agent.provider)
        .bind(&agent.model)
        .bind(agent.cli_path.to_string_lossy().as_ref())
        .bind(serialize_json(&agent.capabilities)?)
        .bind(agent.health_check.as_deref())
        .bind(agent.status)
        .bind(agent.id)
        .execute(&self.pool)
        .await
        .map_err(db_write_err)?;

        ensure_rows_affected(result)
    }

    pub async fn delete_agent(&self, agent_id: AgentId) -> Result<(), RepositoryError> {
        let result = sqlx::query("DELETE FROM agents WHERE id = ?")
            .bind(agent_id)
            .execute(&self.pool)
            .await
            .map_err(db_write_err)?;

        ensure_rows_affected(result)
    }
}

impl AgentRepository for Database {
    async fn list(&self) -> Result<Vec<Agent>, RepositoryError> {
        self.list_agents().await
    }
    async fn get(&self, id: AgentId) -> Result<Agent, RepositoryError> {
        self.get_agent(id).await
    }
    async fn create(&self, agent: &Agent) -> Result<(), RepositoryError> {
        self.create_agent(agent).await
    }
    async fn update(&self, agent: &Agent) -> Result<(), RepositoryError> {
        self.update_agent(agent).await
    }
    async fn delete(&self, id: AgentId) -> Result<(), RepositoryError> {
        self.delete_agent(id).await
    }
}

fn map_agent(row: &SqliteRow) -> Result<Agent, RepositoryError> {
    Ok(Agent {
        id: row_get(row, "id")?,
        slug: row_get(row, "slug")?,
        name: row_get(row, "name")?,
        adapter_kind: row_get(row, "adapter_kind")?,
        provider: row_get(row, "provider")?,
        model: row_get(row, "model")?,
        cli_path: PathBuf::from(row_get::<String>(row, "cli_path")?),
        capabilities: row_get_json(row, "capabilities")?,
        health_check: row_get(row, "health_check")?,
        status: row_get(row, "status")?,
    })
}
