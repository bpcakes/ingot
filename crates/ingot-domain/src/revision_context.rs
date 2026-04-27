use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::commit_oid::CommitOid;
use crate::ids::{ItemRevisionId, JobId};
use crate::job::OutcomeClass;
use crate::step_id::StepId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionContextResultSummary {
    pub job_id: JobId,
    pub schema_version: String,
    pub outcome: OutcomeClass,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionContextAcceptedResultRef {
    pub job_id: JobId,
    pub step_id: StepId,
    pub schema_version: String,
    pub outcome: OutcomeClass,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionContextSummary {
    pub updated_at: DateTime<Utc>,
    pub changed_paths: Vec<String>,
    pub latest_validation: Option<RevisionContextResultSummary>,
    pub latest_review: Option<RevisionContextResultSummary>,
    pub accepted_result_refs: Vec<RevisionContextAcceptedResultRef>,
    pub operator_notes_excerpt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionContextPayload {
    pub authoring_head_commit_oid: Option<CommitOid>,
    pub changed_paths: Vec<String>,
    pub latest_validation: Option<RevisionContextResultSummary>,
    pub latest_review: Option<RevisionContextResultSummary>,
    pub accepted_result_refs: Vec<RevisionContextAcceptedResultRef>,
    pub operator_notes_excerpt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionContext {
    pub item_revision_id: ItemRevisionId,
    pub schema_version: String,
    pub payload: RevisionContextPayload,
    pub updated_from_job_id: Option<JobId>,
    pub updated_at: DateTime<Utc>,
}

impl From<&RevisionContext> for RevisionContextSummary {
    fn from(context: &RevisionContext) -> Self {
        Self {
            updated_at: context.updated_at,
            changed_paths: context.payload.changed_paths.clone(),
            latest_validation: context.payload.latest_validation.clone(),
            latest_review: context.payload.latest_review.clone(),
            accepted_result_refs: context.payload.accepted_result_refs.clone(),
            operator_notes_excerpt: context.payload.operator_notes_excerpt.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn revision_context_summary_from_context_uses_payload_and_row_timestamp() {
        let updated_at = Utc.with_ymd_and_hms(2025, 1, 2, 3, 4, 5).unwrap();
        let validation = RevisionContextResultSummary {
            job_id: JobId::from_uuid(Uuid::from_u128(1)),
            schema_version: "validation:v1".into(),
            outcome: OutcomeClass::Clean,
            summary: "clean".into(),
        };
        let accepted_ref = RevisionContextAcceptedResultRef {
            job_id: JobId::from_uuid(Uuid::from_u128(2)),
            step_id: StepId::ValidateCandidateInitial,
            schema_version: "validation:v1".into(),
            outcome: OutcomeClass::Clean,
            summary: "accepted".into(),
        };
        let context = RevisionContext {
            item_revision_id: ItemRevisionId::from_uuid(Uuid::from_u128(3)),
            schema_version: "revision_context:v1".into(),
            payload: RevisionContextPayload {
                authoring_head_commit_oid: None,
                changed_paths: vec!["src/lib.rs".into()],
                latest_validation: Some(validation),
                latest_review: None,
                accepted_result_refs: vec![accepted_ref],
                operator_notes_excerpt: Some("note".into()),
            },
            updated_from_job_id: Some(JobId::from_uuid(Uuid::from_u128(4))),
            updated_at,
        };

        let summary = RevisionContextSummary::from(&context);

        assert_eq!(summary.updated_at, updated_at);
        assert_eq!(summary.changed_paths, vec!["src/lib.rs"]);
        assert_eq!(
            summary
                .latest_validation
                .as_ref()
                .map(|result| result.job_id),
            Some(JobId::from_uuid(Uuid::from_u128(1)))
        );
        assert!(summary.latest_review.is_none());
        assert_eq!(summary.accepted_result_refs.len(), 1);
        assert_eq!(summary.operator_notes_excerpt.as_deref(), Some("note"));
    }
}
