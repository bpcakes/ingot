use ingot_domain::commit_oid::CommitOid;
use ingot_domain::ids::{AgentId, WorkspaceId};
use ingot_domain::job::{Job, JobInput, JobState, JobStateParts};
use ingot_domain::lease_owner_id::LeaseOwnerId;
use ingot_domain::ports::RepositoryError;
use sqlx::sqlite::SqliteRow;

use crate::store::helpers::{StoreDecodeError, row_get, row_get_optional_json};

pub(super) fn encode_job_input(
    job_input: &JobInput,
) -> (&'static str, Option<CommitOid>, Option<CommitOid>) {
    match job_input {
        JobInput::None => ("none", None, None),
        JobInput::AuthoringHead { head_commit_oid } => {
            ("authoring_head", None, Some(head_commit_oid.clone()))
        }
        JobInput::CandidateSubject {
            base_commit_oid,
            head_commit_oid,
        } => (
            "candidate_subject",
            Some(base_commit_oid.clone()),
            Some(head_commit_oid.clone()),
        ),
        JobInput::IntegratedSubject {
            base_commit_oid,
            head_commit_oid,
        } => (
            "integrated_subject",
            Some(base_commit_oid.clone()),
            Some(head_commit_oid.clone()),
        ),
    }
}

fn decode_job_input(
    kind: String,
    base_commit_oid: Option<CommitOid>,
    head_commit_oid: Option<CommitOid>,
) -> Result<JobInput, StoreDecodeError> {
    match kind.as_str() {
        "none" => Ok(JobInput::None),
        "authoring_head" => head_commit_oid
            .map(JobInput::authoring_head)
            .ok_or_else(|| StoreDecodeError::Json("authoring_head job_input missing head".into())),
        "candidate_subject" => match (base_commit_oid, head_commit_oid) {
            (Some(base_commit_oid), Some(head_commit_oid)) => Ok(JobInput::candidate_subject(
                base_commit_oid,
                head_commit_oid,
            )),
            _ => Err(StoreDecodeError::Json(
                "candidate_subject job_input missing base or head".into(),
            )),
        },
        "integrated_subject" => match (base_commit_oid, head_commit_oid) {
            (Some(base_commit_oid), Some(head_commit_oid)) => Ok(JobInput::integrated_subject(
                base_commit_oid,
                head_commit_oid,
            )),
            _ => Err(StoreDecodeError::Json(
                "integrated_subject job_input missing base or head".into(),
            )),
        },
        _ => Err(StoreDecodeError::Json(format!(
            "unknown job_input_kind: {kind}"
        ))),
    }
}

pub(super) fn map_job(row: &SqliteRow) -> Result<Job, RepositoryError> {
    use ingot_domain::job::{JobStatus, OutcomeClass};

    let status: JobStatus = row_get(row, "status")?;
    let outcome_class: Option<OutcomeClass> = row_get(row, "outcome_class")?;
    let workspace_id: Option<WorkspaceId> = row_get(row, "workspace_id")?;
    let agent_id: Option<AgentId> = row_get(row, "agent_id")?;
    let prompt_snapshot: Option<String> = row_get(row, "prompt_snapshot")?;
    let phase_template_digest: Option<String> = row_get(row, "phase_template_digest")?;
    let output_commit_oid: Option<CommitOid> = row_get(row, "output_commit_oid")?;
    let result_schema_version: Option<String> = row_get(row, "result_schema_version")?;
    let result_payload: Option<serde_json::Value> = row_get_optional_json(row, "result_payload")?;
    let process_pid: Option<u32> =
        row_get::<Option<i64>>(row, "process_pid")?.map(|value| value as u32);
    let lease_owner_id: Option<LeaseOwnerId> = row_get(row, "lease_owner_id")?;
    let heartbeat_at: Option<chrono::DateTime<chrono::Utc>> = row_get(row, "heartbeat_at")?;
    let lease_expires_at: Option<chrono::DateTime<chrono::Utc>> = row_get(row, "lease_expires_at")?;
    let error_code: Option<String> = row_get(row, "error_code")?;
    let error_message: Option<String> = row_get(row, "error_message")?;
    let started_at: Option<chrono::DateTime<chrono::Utc>> = row_get(row, "started_at")?;
    let ended_at: Option<chrono::DateTime<chrono::Utc>> = row_get(row, "ended_at")?;

    let job_input_kind = row_get(row, "job_input_kind")?;
    let input_base_commit_oid = row_get(row, "input_base_commit_oid")?;
    let input_head_commit_oid = row_get(row, "input_head_commit_oid")?;
    let job_input = decode_job_input(job_input_kind, input_base_commit_oid, input_head_commit_oid)
        .map_err(|error| RepositoryError::Database(Box::new(error)))?;

    let state = JobState::from_parts(
        status,
        JobStateParts {
            outcome_class,
            workspace_id,
            agent_id,
            prompt_snapshot,
            phase_template_digest,
            output_commit_oid,
            result_schema_version,
            result_payload,
            process_pid,
            lease_owner_id,
            heartbeat_at,
            lease_expires_at,
            error_code,
            error_message,
            started_at,
            ended_at,
        },
    )
    .map_err(|error| RepositoryError::Database(error.into()))?;

    Ok(Job {
        id: row_get(row, "id")?,
        project_id: row_get(row, "project_id")?,
        item_id: row_get(row, "item_id")?,
        item_revision_id: row_get(row, "item_revision_id")?,
        step_id: row_get(row, "step_id")?,
        semantic_attempt_no: row_get::<i64>(row, "semantic_attempt_no")? as u32,
        retry_no: row_get::<i64>(row, "retry_no")? as u32,
        supersedes_job_id: row_get(row, "supersedes_job_id")?,
        phase_kind: row_get(row, "phase_kind")?,
        workspace_kind: row_get(row, "workspace_kind")?,
        execution_permission: row_get(row, "execution_permission")?,
        context_policy: row_get(row, "context_policy")?,
        phase_template_slug: row_get(row, "phase_template_slug")?,
        job_input,
        output_artifact_kind: row_get(row, "output_artifact_kind")?,
        created_at: row_get(row, "created_at")?,
        state,
    })
}
