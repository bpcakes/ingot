use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::agent_model::AgentModel;
use crate::ids::AgentId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    ClaudeCode,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentProvider {
    Anthropic,
    #[serde(rename = "openai")]
    OpenAi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCapability {
    ReadOnlyJobs,
    MutatingJobs,
    StructuredOutput,
    StreamingProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Available,
    Unavailable,
    Probing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: AgentId,
    pub slug: String,
    pub name: String,
    pub adapter_kind: AdapterKind,
    pub provider: AgentProvider,
    pub model: AgentModel,
    pub cli_path: PathBuf,
    pub capabilities: Vec<AgentCapability>,
    pub health_check: Option<String>,
    pub status: AgentStatus,
}
