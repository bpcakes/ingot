use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{FindingId, ItemId, ItemRevisionId, ProjectId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    Change,
    Bug,
    Investigation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParkingState {
    Active,
    Deferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoneReason {
    Completed,
    Dismissed,
    Invalidated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionSource {
    SystemCommand,
    ApprovalCommand,
    ManualCommand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    NotRequired,
    NotRequested,
    Pending,
    Approved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EscalationReason {
    CandidateReworkBudgetExhausted,
    IntegrationReworkBudgetExhausted,
    ConvergenceConflict,
    CheckoutSyncBlocked,
    StepFailed,
    ProtocolViolation,
    ManualDecisionRequired,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Critical,
    Major,
    Minor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowVersion {
    #[serde(rename = "delivery:v1")]
    DeliveryV1,
    #[serde(rename = "investigation:v1")]
    InvestigationV1,
}

/// Item lifecycle state. Encodes the TLA+ invariant `DoneImpliesQuiescent`:
/// a Done item always carries its reason, resolution source, and closure timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "lifecycle_state", rename_all = "snake_case")]
pub enum Lifecycle {
    Open,
    Done {
        #[serde(rename = "done_reason")]
        reason: DoneReason,
        #[serde(rename = "resolution_source")]
        source: ResolutionSource,
        closed_at: DateTime<Utc>,
    },
}

impl Lifecycle {
    #[must_use]
    pub fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }

    #[must_use]
    pub fn is_done(self) -> bool {
        matches!(self, Self::Done { .. })
    }

    #[must_use]
    pub fn done_reason(self) -> Option<DoneReason> {
        match self {
            Self::Done { reason, .. } => Some(reason),
            Self::Open => None,
        }
    }

    #[must_use]
    pub fn resolution_source(self) -> Option<ResolutionSource> {
        match self {
            Self::Done { source, .. } => Some(source),
            Self::Open => None,
        }
    }

    #[must_use]
    pub fn closed_at(self) -> Option<DateTime<Utc>> {
        match self {
            Self::Done { closed_at, .. } => Some(closed_at),
            Self::Open => None,
        }
    }
}

/// Item escalation state. When escalated, an escalation reason is always present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "escalation_state", rename_all = "snake_case")]
pub enum Escalation {
    None,
    OperatorRequired {
        #[serde(rename = "escalation_reason")]
        reason: EscalationReason,
    },
}

impl Escalation {
    #[must_use]
    pub fn is_escalated(self) -> bool {
        matches!(self, Self::OperatorRequired { .. })
    }

    #[must_use]
    pub fn reason(self) -> Option<EscalationReason> {
        match self {
            Self::OperatorRequired { reason } => Some(reason),
            Self::None => None,
        }
    }
}

/// Item origin. A promoted-finding origin always carries its source finding ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "origin_kind", rename_all = "snake_case")]
pub enum Origin {
    Manual,
    PromotedFinding {
        #[serde(rename = "origin_finding_id")]
        finding_id: FindingId,
    },
}

impl Origin {
    #[must_use]
    pub fn is_promoted_finding(self) -> bool {
        matches!(self, Self::PromotedFinding { .. })
    }

    #[must_use]
    pub fn finding_id(self) -> Option<FindingId> {
        match self {
            Self::PromotedFinding { finding_id } => Some(finding_id),
            Self::Manual => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: ItemId,
    pub project_id: ProjectId,
    pub classification: Classification,
    pub workflow_version: WorkflowVersion,
    #[serde(flatten)]
    pub lifecycle: Lifecycle,
    pub parking_state: ParkingState,
    pub approval_state: ApprovalState,
    #[serde(flatten)]
    pub escalation: Escalation,
    pub current_revision_id: ItemRevisionId,
    #[serde(flatten)]
    pub origin: Origin,
    pub priority: Priority,
    pub labels: Vec<String>,
    pub operator_notes: Option<String>,
    pub sort_key: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
