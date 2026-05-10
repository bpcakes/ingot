use std::path::PathBuf;
use std::sync::Arc;

use crate::UseCaseError;
use crate::authoring_history::selected_prepared_convergence;
use crate::finding::extract_findings;
use ingot_domain::commit_oid::CommitOid;
use ingot_domain::ids::JobId;
use ingot_domain::item::{ApprovalState, Item};
use ingot_domain::job::{Job, JobStatus, OutcomeClass, OutputArtifactKind};
use ingot_domain::ports::{
    ConflictKind, GitPortError, JobCompletionContext, JobCompletionGitPort, JobCompletionMutation,
    JobCompletionRepository, PreparedConvergenceGuard, ProjectMutationLockPort, RepositoryError,
    TargetRefHoldError,
};
use ingot_domain::project::Project;
use ingot_domain::revision::ItemRevision;
use ingot_domain::step_id::StepId;
use serde_json::Value;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct CompleteJobCommand {
    pub job_id: JobId,
    pub outcome_class: OutcomeClass,
    pub result_schema_version: Option<String>,
    pub result_payload: Option<Value>,
    pub output_commit_oid: Option<CommitOid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompleteJobResult {
    pub finding_count: usize,
}

#[derive(Debug)]
pub enum CompleteJobError {
    BadRequest { code: &'static str, message: String },
    UseCase(UseCaseError),
}

impl From<UseCaseError> for CompleteJobError {
    fn from(error: UseCaseError) -> Self {
        Self::UseCase(error)
    }
}

#[derive(Debug)]
struct JobCompletionPlan {
    outcome_class: OutcomeClass,
    result_schema_version: Option<String>,
    result_payload: Option<Value>,
    output_commit_oid: Option<CommitOid>,
    findings: Vec<ingot_domain::finding::Finding>,
    prepared_convergence_guard: Option<PreparedConvergenceGuard>,
}

#[derive(Debug)]
struct NormalizedCompleteJobCommand {
    outcome_class: OutcomeClass,
    result_schema_version: Option<String>,
    result_payload: Option<Value>,
    output_commit_oid: Option<CommitOid>,
}

enum LoadedCompletionContext {
    Ready(Box<JobCompletionContext>),
    Retry(CompleteJobResult),
}

#[derive(Clone)]
pub struct CompleteJobService<R, G, L> {
    repository: R,
    git: G,
    project_locks: L,
    repo_path_resolver: Arc<dyn Fn(&Project) -> PathBuf + Send + Sync>,
}

impl<R, G, L> CompleteJobService<R, G, L> {
    pub fn new(repository: R, git: G, project_locks: L) -> Self {
        Self::with_repo_path_resolver(
            repository,
            git,
            project_locks,
            Arc::new(|project: &Project| project.path.clone()),
        )
    }

    pub fn with_repo_path_resolver(
        repository: R,
        git: G,
        project_locks: L,
        repo_path_resolver: Arc<dyn Fn(&Project) -> PathBuf + Send + Sync>,
    ) -> Self {
        Self {
            repository,
            git,
            project_locks,
            repo_path_resolver,
        }
    }
}

impl<R, G, L> CompleteJobService<R, G, L>
where
    R: JobCompletionRepository,
    G: JobCompletionGitPort,
    L: ProjectMutationLockPort,
{
    pub async fn execute(
        &self,
        command: CompleteJobCommand,
    ) -> Result<CompleteJobResult, CompleteJobError> {
        let mut context = match self
            .load_completion_context(command.job_id, &command)
            .await?
        {
            LoadedCompletionContext::Ready(context) => *context,
            LoadedCompletionContext::Retry(result) => return Ok(result),
        };

        let project_lock = if requires_project_serialization(&context.job, command.outcome_class) {
            Some(
                self.project_locks
                    .acquire_project_mutation(context.project.id)
                    .await,
            )
        } else {
            None
        };

        if project_lock.is_some() {
            context = match self
                .load_completion_context(command.job_id, &command)
                .await?
            {
                LoadedCompletionContext::Ready(context) => *context,
                LoadedCompletionContext::Retry(result) => return Ok(result),
            };
        }

        let normalized_command = normalize_completion_command(&context.job, &command)?;
        let plan = self
            .prepare_job_completion(&context, normalized_command)
            .await?;
        let JobCompletionPlan {
            outcome_class,
            result_schema_version,
            result_payload,
            output_commit_oid,
            findings,
            prepared_convergence_guard,
        } = plan;
        let finding_count = findings.len();
        let repo_path = (self.repo_path_resolver)(&context.project);
        let ref_hold = if let Some(guard) = prepared_convergence_guard.as_ref() {
            Some(
                self.git
                    .verify_and_hold_target_ref(
                        repo_path.as_path(),
                        &guard.target_ref,
                        &guard.expected_target_head_oid,
                    )
                    .await
                    .map_err(map_target_ref_hold_error)?,
            )
        } else {
            None
        };

        let result = self
            .repository
            .apply_job_completion(JobCompletionMutation {
                job_id: context.job.id,
                item_id: context.item.id,
                expected_item_revision_id: context.job.item_revision_id,
                outcome_class,
                clear_item_escalation: should_clear_item_escalation_on_success(
                    &context.item,
                    &context.job,
                ),
                result_schema_version,
                result_payload,
                output_commit_oid,
                findings,
                prepared_convergence_guard,
            })
            .await
            .map_err(map_completion_apply_error);

        let release_result = if let Some(hold) = ref_hold {
            self.git.release_hold(hold).await.map_err(|error| {
                warn!(
                    ?error,
                    job_id = %context.job.id,
                    "failed to release target ref hold after job completion"
                );
                map_git_port_error(error)
            })
        } else {
            Ok(())
        };

        drop(project_lock);
        result?;
        release_result?;

        Ok(CompleteJobResult { finding_count })
    }

    async fn try_completed_job_retry(
        &self,
        job_id: JobId,
        job: &Job,
        command: &CompleteJobCommand,
    ) -> Result<Option<CompleteJobResult>, CompleteJobError> {
        if !completed_job_retry_allowed(job) {
            return Ok(None);
        }

        let Some(completed) = self
            .repository
            .load_completed_job_completion(job_id)
            .await
            .map_err(map_repository_error)?
        else {
            return Ok(None);
        };

        if !completed_job_matches_retry_command(&completed.job, command) {
            return Ok(None);
        }

        Ok(Some(CompleteJobResult {
            finding_count: completed.finding_count,
        }))
    }

    async fn load_completion_context(
        &self,
        job_id: JobId,
        command: &CompleteJobCommand,
    ) -> Result<LoadedCompletionContext, CompleteJobError> {
        let context = self
            .repository
            .load_job_completion_context(job_id)
            .await
            .map_err(map_repository_error)?;
        if let Some(result) = self
            .try_completed_job_retry(job_id, &context.job, command)
            .await?
        {
            return Ok(LoadedCompletionContext::Retry(result));
        }

        validate_completion_context(&context)?;
        Ok(LoadedCompletionContext::Ready(Box::new(context)))
    }

    async fn prepare_job_completion(
        &self,
        context: &JobCompletionContext,
        command: NormalizedCompleteJobCommand,
    ) -> Result<JobCompletionPlan, CompleteJobError> {
        match context.job.output_artifact_kind {
            OutputArtifactKind::Commit => {
                let output_commit_oid = command.output_commit_oid.ok_or_else(|| {
                    missing_normalized_completion_field("commit", "output_commit_oid")
                })?;

                let commit_is_present = self
                    .git
                    .commit_exists(
                        (self.repo_path_resolver)(&context.project).as_path(),
                        &output_commit_oid,
                    )
                    .await
                    .map_err(map_git_port_error)?;
                if !commit_is_present {
                    return Err(CompleteJobError::BadRequest {
                        code: "missing_output_commit_oid",
                        message:
                            "output_commit_oid does not resolve to a commit in the project repository"
                                .into(),
                    });
                }

                Ok(JobCompletionPlan {
                    outcome_class: command.outcome_class,
                    result_schema_version: None,
                    result_payload: None,
                    output_commit_oid: Some(output_commit_oid),
                    findings: vec![],
                    prepared_convergence_guard: None,
                })
            }
            OutputArtifactKind::ValidationReport
            | OutputArtifactKind::ReviewReport
            | OutputArtifactKind::FindingReport
            | OutputArtifactKind::InvestigationReport => {
                let result_schema_version = command.result_schema_version.ok_or_else(|| {
                    missing_normalized_completion_field("report", "schema version")
                })?;
                let result_payload = command
                    .result_payload
                    .ok_or_else(|| missing_normalized_completion_field("report", "payload"))?;

                let mut completed_job = context.job.clone();
                completed_job.complete(
                    command.outcome_class,
                    chrono::Utc::now(),
                    None,
                    Some(result_schema_version.clone()),
                    Some(result_payload.clone()),
                );

                let extracted =
                    extract_findings(&context.item, &completed_job, &context.convergences)?;
                if extracted.outcome_class != command.outcome_class {
                    return Err(CompleteJobError::BadRequest {
                        code: "outcome_mismatch",
                        message: format!(
                            "Requested outcome_class={} does not match report outcome {}",
                            outcome_class_name(command.outcome_class),
                            outcome_class_name(extracted.outcome_class)
                        ),
                    });
                }

                let prepared_convergence_guard = prepared_convergence_guard(
                    &context.item,
                    &context.revision,
                    &completed_job,
                    &context.convergences,
                )?;

                Ok(JobCompletionPlan {
                    outcome_class: command.outcome_class,
                    result_schema_version: Some(result_schema_version),
                    result_payload: Some(result_payload),
                    output_commit_oid: None,
                    findings: extracted.findings,
                    prepared_convergence_guard,
                })
            }
            OutputArtifactKind::None => Ok(JobCompletionPlan {
                outcome_class: command.outcome_class,
                result_schema_version: None,
                result_payload: None,
                output_commit_oid: None,
                findings: vec![],
                prepared_convergence_guard: None,
            }),
        }
    }
}

fn normalize_completion_command(
    job: &Job,
    command: &CompleteJobCommand,
) -> Result<NormalizedCompleteJobCommand, CompleteJobError> {
    match job.output_artifact_kind {
        OutputArtifactKind::Commit => {
            if command.outcome_class != OutcomeClass::Clean {
                return Err(CompleteJobError::BadRequest {
                    code: "invalid_outcome_class",
                    message: "Commit-producing jobs may only complete with outcome_class=clean"
                        .into(),
                });
            }

            if command.result_schema_version.is_some() || command.result_payload.is_some() {
                return Err(CompleteJobError::BadRequest {
                    code: "invalid_completion_artifact",
                    message: "Commit-producing jobs must not include structured report payloads"
                        .into(),
                });
            }

            let output_commit_oid = command
                .output_commit_oid
                .clone()
                .filter(|value| !value.as_str().trim().is_empty())
                .ok_or_else(|| CompleteJobError::BadRequest {
                    code: "missing_output_commit_oid",
                    message: "Commit-producing jobs must include output_commit_oid".into(),
                })?;

            Ok(NormalizedCompleteJobCommand {
                outcome_class: command.outcome_class,
                result_schema_version: None,
                result_payload: None,
                output_commit_oid: Some(output_commit_oid),
            })
        }
        OutputArtifactKind::ValidationReport
        | OutputArtifactKind::ReviewReport
        | OutputArtifactKind::FindingReport
        | OutputArtifactKind::InvestigationReport => {
            if command.output_commit_oid.is_some() {
                return Err(CompleteJobError::BadRequest {
                    code: "invalid_completion_artifact",
                    message: "Report-producing jobs must not include output_commit_oid".into(),
                });
            }

            let expected_schema_version = expected_schema_version(job.output_artifact_kind);
            let result_schema_version = command.result_schema_version.clone().ok_or_else(|| {
                CompleteJobError::BadRequest {
                    code: "missing_result_schema_version",
                    message: "Report-producing jobs must include result_schema_version".into(),
                }
            })?;
            let result_payload =
                command
                    .result_payload
                    .clone()
                    .ok_or_else(|| CompleteJobError::BadRequest {
                        code: "missing_result_payload",
                        message: "Report-producing jobs must include result_payload".into(),
                    })?;

            if result_schema_version != expected_schema_version {
                return Err(CompleteJobError::BadRequest {
                    code: "invalid_result_schema_version",
                    message: format!(
                        "Expected result_schema_version={}, got {}",
                        expected_schema_version, result_schema_version
                    ),
                });
            }

            if !matches!(
                command.outcome_class,
                OutcomeClass::Clean | OutcomeClass::Findings
            ) {
                return Err(CompleteJobError::BadRequest {
                    code: "invalid_outcome_class",
                    message:
                        "Report-producing jobs may only complete with outcome_class=clean or findings"
                            .into(),
                });
            }

            Ok(NormalizedCompleteJobCommand {
                outcome_class: command.outcome_class,
                result_schema_version: Some(result_schema_version),
                result_payload: Some(result_payload),
                output_commit_oid: None,
            })
        }
        OutputArtifactKind::None => {
            if command.result_schema_version.is_some()
                || command.result_payload.is_some()
                || command.output_commit_oid.is_some()
            {
                return Err(CompleteJobError::BadRequest {
                    code: "invalid_completion_artifact",
                    message: "Jobs without output artifacts must not include completion artifacts"
                        .into(),
                });
            }

            if command.outcome_class != OutcomeClass::Clean {
                return Err(CompleteJobError::BadRequest {
                    code: "invalid_outcome_class",
                    message: "Artifact-free jobs may only complete with outcome_class=clean".into(),
                });
            }

            Ok(NormalizedCompleteJobCommand {
                outcome_class: command.outcome_class,
                result_schema_version: None,
                result_payload: None,
                output_commit_oid: None,
            })
        }
    }
}

fn completed_job_matches_retry_command(job: &Job, command: &CompleteJobCommand) -> bool {
    job.state.status() == JobStatus::Completed
        && job.state.outcome_class() == Some(command.outcome_class)
        && job.state.result_schema_version() == command.result_schema_version.as_deref()
        && job.state.result_payload() == command.result_payload.as_ref()
        && job.state.output_commit_oid() == command.output_commit_oid.as_ref()
}

fn completed_job_retry_allowed(job: &Job) -> bool {
    job.state.status() == JobStatus::Completed && !completed_job_uses_target_ref_hold(job)
}

fn completed_job_uses_target_ref_hold(job: &Job) -> bool {
    job.step_id == StepId::ValidateIntegrated
        && job.state.outcome_class() == Some(OutcomeClass::Clean)
}

fn validate_completion_context(context: &JobCompletionContext) -> Result<(), CompleteJobError> {
    if !context.job.state.is_active() {
        return Err(UseCaseError::JobNotActive.into());
    }

    if context.job.item_revision_id != context.item.current_revision_id {
        return Err(UseCaseError::ProtocolViolation(
            "job completion does not match the current item revision".into(),
        )
        .into());
    }

    Ok(())
}

fn requires_project_serialization(job: &Job, outcome_class: OutcomeClass) -> bool {
    let _ = (job, outcome_class);
    true
}

fn desired_completion_approval_state(
    item: &Item,
    revision: &ItemRevision,
    job: &Job,
) -> Option<ApprovalState> {
    if job.step_id != StepId::ValidateIntegrated
        || job.state.outcome_class() != Some(OutcomeClass::Clean)
    {
        return None;
    }

    let approval_state = crate::item::pending_approval_state(revision.approval_policy);

    if item.approval_state == approval_state {
        None
    } else {
        Some(approval_state)
    }
}

fn should_clear_item_escalation_on_success(item: &Item, job: &Job) -> bool {
    crate::dispatch::should_clear_item_escalation_on_success(item, job)
}

fn prepared_convergence_guard(
    item: &Item,
    revision: &ItemRevision,
    job: &Job,
    convergences: &[ingot_domain::convergence::Convergence],
) -> Result<Option<PreparedConvergenceGuard>, CompleteJobError> {
    if job.step_id != StepId::ValidateIntegrated
        || job.state.outcome_class() != Some(OutcomeClass::Clean)
    {
        return Ok(None);
    }

    let Some(prepared_convergence) =
        selected_prepared_convergence(job.item_revision_id, convergences)
    else {
        return Err(UseCaseError::PreparedConvergenceMissing.into());
    };

    let Some(expected_target_oid) = prepared_convergence.state.input_target_commit_oid() else {
        return Err(UseCaseError::PreparedConvergenceStale.into());
    };

    Ok(Some(PreparedConvergenceGuard {
        convergence_id: prepared_convergence.id,
        item_revision_id: job.item_revision_id,
        target_ref: prepared_convergence.target_ref.clone(),
        expected_target_head_oid: expected_target_oid.clone(),
        next_approval_state: desired_completion_approval_state(item, revision, job),
    }))
}

fn expected_schema_version(output_artifact_kind: OutputArtifactKind) -> &'static str {
    ingot_agent_protocol::report::schema_version(output_artifact_kind).unwrap_or("")
}

fn outcome_class_name(outcome_class: OutcomeClass) -> &'static str {
    outcome_class.as_str()
}

fn map_repository_error(error: RepositoryError) -> CompleteJobError {
    UseCaseError::Repository(error).into()
}

fn map_completion_apply_error(error: RepositoryError) -> CompleteJobError {
    match error {
        RepositoryError::Conflict(ConflictKind::JobNotActive) => UseCaseError::JobNotActive.into(),
        RepositoryError::Conflict(ConflictKind::JobRevisionStale) => {
            UseCaseError::ProtocolViolation(
                "job completion does not match the current item revision".into(),
            )
            .into()
        }
        RepositoryError::Conflict(ConflictKind::PreparedConvergenceMissing) => {
            UseCaseError::PreparedConvergenceMissing.into()
        }
        RepositoryError::Conflict(ConflictKind::PreparedConvergenceStale) => {
            UseCaseError::PreparedConvergenceStale.into()
        }
        other => map_repository_error(other),
    }
}

fn map_git_port_error(error: GitPortError) -> CompleteJobError {
    UseCaseError::Internal(error.to_string()).into()
}

fn missing_normalized_completion_field(
    artifact_kind: &'static str,
    field: &'static str,
) -> CompleteJobError {
    UseCaseError::Internal(format!(
        "{artifact_kind} completion missing {field} after normalization"
    ))
    .into()
}

fn map_target_ref_hold_error(error: TargetRefHoldError) -> CompleteJobError {
    match error {
        TargetRefHoldError::Stale => UseCaseError::PreparedConvergenceStale.into(),
        TargetRefHoldError::Internal(message) => UseCaseError::Internal(message).into(),
    }
}

pub(crate) use termination::map_finish_non_success_error;
pub use termination::{JobTerminationResult, cancel_job, expire_job, fail_job};

mod termination {
    use chrono::Utc;
    use ingot_domain::activity::{Activity, ActivityEventType, ActivitySubject};
    use ingot_domain::ids::{ActivityId, ItemId, ItemRevisionId, JobId, ProjectId, WorkspaceId};
    use ingot_domain::item::{EscalationReason, Item};
    use ingot_domain::job::{Job, JobStatus, OutcomeClass};
    use ingot_domain::ports::{
        ActivityRepository, ConflictKind, FinishJobNonSuccessParams, JobRepository,
        RepositoryError, WorkspaceRepository,
    };
    use ingot_domain::workspace::WorkspaceStatus;

    use crate::UseCaseError;
    use crate::dispatch::{failure_escalation_reason, failure_status};

    /// Result returned after a job termination (cancel, fail, expire).
    /// Callers use this to know what infrastructure side effects to perform
    /// (e.g., refresh_revision_context, workspace filesystem cleanup).
    #[derive(Debug, Clone)]
    pub struct JobTerminationResult {
        pub job_id: JobId,
        pub project_id: ProjectId,
        pub item_id: ItemId,
        pub revision_id: ItemRevisionId,
        pub released_workspace_id: Option<WorkspaceId>,
        pub escalation_reason: Option<EscalationReason>,
    }

    struct NonSuccessTermination {
        status: JobStatus,
        outcome_class: Option<OutcomeClass>,
        error_code: Option<String>,
        error_message: Option<String>,
        escalation_reason: Option<EscalationReason>,
        revision_stale_message: &'static str,
        activities: Vec<TerminationActivity>,
    }

    struct TerminationActivity {
        event_type: ActivityEventType,
        subject: ActivitySubject,
        payload: serde_json::Value,
    }

    /// Cancel an active job. Sets status to Cancelled, releases workspace (to `target_workspace_status`),
    /// and appends a JobCancelled activity.
    pub async fn cancel_job<J, W, A>(
        job_repo: &J,
        workspace_repo: &W,
        activity_repo: &A,
        job: &Job,
        item: &Item,
        cancel_reason: &str,
        target_workspace_status: WorkspaceStatus,
    ) -> Result<JobTerminationResult, UseCaseError>
    where
        J: JobRepository,
        W: WorkspaceRepository,
        A: ActivityRepository,
    {
        terminate_non_success_job(
            job_repo,
            workspace_repo,
            activity_repo,
            job,
            item,
            target_workspace_status,
            || {
                Ok(NonSuccessTermination {
                    status: JobStatus::Cancelled,
                    outcome_class: Some(OutcomeClass::Cancelled),
                    error_code: Some(cancel_reason.into()),
                    error_message: None,
                    escalation_reason: None,
                    revision_stale_message: "job cancellation does not match the current item revision",
                    activities: vec![TerminationActivity {
                        event_type: ActivityEventType::JobCancelled,
                        subject: ActivitySubject::Job(job.id),
                        payload: serde_json::json!({ "item_id": item.id, "reason": cancel_reason }),
                    }],
                })
            },
        )
        .await
    }

    /// Fail an active job with a given outcome class. Sets the appropriate terminal status,
    /// releases workspace, appends JobFailed + optional ItemEscalated activities.
    #[allow(clippy::too_many_arguments)]
    pub async fn fail_job<J, W, A>(
        job_repo: &J,
        workspace_repo: &W,
        activity_repo: &A,
        job: &Job,
        item: &Item,
        outcome_class: OutcomeClass,
        error_code: Option<String>,
        error_message: Option<String>,
        target_workspace_status: WorkspaceStatus,
    ) -> Result<JobTerminationResult, UseCaseError>
    where
        J: JobRepository,
        W: WorkspaceRepository,
        A: ActivityRepository,
    {
        terminate_non_success_job(
            job_repo,
            workspace_repo,
            activity_repo,
            job,
            item,
            target_workspace_status,
            || {
                let status = failure_status(outcome_class).ok_or_else(|| {
                    UseCaseError::ProtocolViolation(
                        "Failure endpoints only accept transient_failure, terminal_failure, protocol_violation, or cancelled".into(),
                    )
                })?;
                let escalation_reason = failure_escalation_reason(job, outcome_class);

                let event_type = if outcome_class == OutcomeClass::Cancelled {
                    ActivityEventType::JobCancelled
                } else {
                    ActivityEventType::JobFailed
                };

                let mut activities = Vec::new();
                if let Some(reason) = escalation_reason {
                    activities.push(TerminationActivity {
                        event_type: ActivityEventType::ItemEscalated,
                        subject: ActivitySubject::Item(item.id),
                        payload: serde_json::json!({ "reason": reason }),
                    });
                }
                activities.push(TerminationActivity {
                    event_type,
                    subject: ActivitySubject::Job(job.id),
                    payload: serde_json::json!({ "item_id": item.id, "error_code": error_code.as_deref() }),
                });

                Ok(NonSuccessTermination {
                    status,
                    outcome_class: Some(outcome_class),
                    error_code,
                    error_message,
                    escalation_reason,
                    revision_stale_message: "job failure does not match the current item revision",
                    activities,
                })
            },
        )
        .await
    }

    /// Expire an active job. Sets status to Expired with TransientFailure outcome.
    pub async fn expire_job<J, W, A>(
        job_repo: &J,
        workspace_repo: &W,
        activity_repo: &A,
        job: &Job,
        item: &Item,
        target_workspace_status: WorkspaceStatus,
    ) -> Result<JobTerminationResult, UseCaseError>
    where
        J: JobRepository,
        W: WorkspaceRepository,
        A: ActivityRepository,
    {
        terminate_non_success_job(
            job_repo,
            workspace_repo,
            activity_repo,
            job,
            item,
            target_workspace_status,
            || {
                Ok(NonSuccessTermination {
                    status: JobStatus::Expired,
                    outcome_class: Some(OutcomeClass::TransientFailure),
                    error_code: Some("job_expired".into()),
                    error_message: None,
                    escalation_reason: None,
                    revision_stale_message: "job expiration does not match the current item revision",
                    activities: vec![TerminationActivity {
                        event_type: ActivityEventType::JobFailed,
                        subject: ActivitySubject::Job(job.id),
                        payload: serde_json::json!({ "item_id": item.id, "error_code": "job_expired" }),
                    }],
                })
            },
        )
        .await
    }

    async fn terminate_non_success_job<J, W, A>(
        job_repo: &J,
        workspace_repo: &W,
        activity_repo: &A,
        job: &Job,
        item: &Item,
        target_workspace_status: WorkspaceStatus,
        build_termination: impl FnOnce() -> Result<NonSuccessTermination, UseCaseError>,
    ) -> Result<JobTerminationResult, UseCaseError>
    where
        J: JobRepository,
        W: WorkspaceRepository,
        A: ActivityRepository,
    {
        if !job.state.is_active() {
            return Err(UseCaseError::JobNotActive);
        }
        let termination = build_termination()?;

        let escalation_reason = termination.escalation_reason;

        job_repo
            .finish_non_success(FinishJobNonSuccessParams {
                job_id: job.id,
                item_id: item.id,
                expected_item_revision_id: job.item_revision_id,
                status: termination.status,
                outcome_class: termination.outcome_class,
                error_code: termination.error_code,
                error_message: termination.error_message,
                escalation_reason,
            })
            .await
            .map_err(|error| {
                map_finish_non_success_error(error, termination.revision_stale_message)
            })?;

        let released_workspace_id = release_workspace(
            workspace_repo,
            job.state.workspace_id(),
            target_workspace_status,
        )
        .await?;

        for activity in termination.activities {
            activity_repo
                .append(&Activity {
                    id: ActivityId::new(),
                    project_id: job.project_id,
                    event_type: activity.event_type,
                    subject: activity.subject,
                    payload: activity.payload,
                    created_at: Utc::now(),
                })
                .await?;
        }

        Ok(JobTerminationResult {
            job_id: job.id,
            project_id: job.project_id,
            item_id: item.id,
            revision_id: job.item_revision_id,
            released_workspace_id,
            escalation_reason,
        })
    }

    /// Release a workspace after job termination: clear current_job_id, set status.
    async fn release_workspace<W: WorkspaceRepository>(
        workspace_repo: &W,
        workspace_id: Option<WorkspaceId>,
        target_status: WorkspaceStatus,
    ) -> Result<Option<WorkspaceId>, UseCaseError> {
        let Some(workspace_id) = workspace_id else {
            return Ok(None);
        };
        let mut workspace = workspace_repo.get(workspace_id).await?;
        workspace.release_to(target_status, Utc::now());
        workspace_repo.update(&workspace).await?;
        Ok(Some(workspace_id))
    }

    pub(crate) fn map_finish_non_success_error(
        error: RepositoryError,
        revision_stale_message: &'static str,
    ) -> UseCaseError {
        match error {
            RepositoryError::Conflict(ConflictKind::JobNotActive) => UseCaseError::JobNotActive,
            RepositoryError::Conflict(ConflictKind::JobRevisionStale) => {
                UseCaseError::ProtocolViolation(revision_stale_message.into())
            }
            other => UseCaseError::Repository(other),
        }
    }

    #[cfg(test)]
    mod tests {
        use std::sync::{Arc, Mutex};

        use ingot_domain::activity::Activity;
        use ingot_domain::ids::{ItemId, ItemRevisionId, JobId, ProjectId, WorkspaceId};
        use ingot_domain::job::{JobStatus, OutcomeClass};
        use ingot_domain::ports::{ConflictKind, RepositoryError, StartJobExecutionParams};
        use ingot_domain::test_support::{JobBuilder, WorkspaceBuilder, nil_item};
        use ingot_domain::workspace::{Workspace, WorkspaceKind, WorkspaceStatus};
        use uuid::Uuid;

        use super::*;

        #[tokio::test]
        async fn cancel_job_maps_job_not_active_conflict() {
            let job = test_job(None);
            let item = nil_item();
            let job_repo = FakeJobRepository::with_finish_error(RepositoryError::Conflict(
                ConflictKind::JobNotActive,
            ));

            let result = cancel_job(
                &job_repo,
                &FakeWorkspaceRepository::default(),
                &FakeActivityRepository,
                &job,
                &item,
                "operator_cancelled",
                WorkspaceStatus::Ready,
            )
            .await;

            assert!(matches!(result, Err(UseCaseError::JobNotActive)));
        }

        #[tokio::test]
        async fn cancel_job_maps_revision_stale_conflict() {
            let job = test_job(None);
            let item = nil_item();
            let job_repo = FakeJobRepository::with_finish_error(RepositoryError::Conflict(
                ConflictKind::JobRevisionStale,
            ));

            let result = cancel_job(
                &job_repo,
                &FakeWorkspaceRepository::default(),
                &FakeActivityRepository,
                &job,
                &item,
                "operator_cancelled",
                WorkspaceStatus::Ready,
            )
            .await;

            assert!(matches!(
                result,
                Err(UseCaseError::ProtocolViolation(message))
                    if message == "job cancellation does not match the current item revision"
            ));
        }

        #[tokio::test]
        async fn fail_job_maps_revision_stale_conflict() {
            let job = test_job(None);
            let item = nil_item();
            let job_repo = FakeJobRepository::with_finish_error(RepositoryError::Conflict(
                ConflictKind::JobRevisionStale,
            ));

            let result = fail_job(
                &job_repo,
                &FakeWorkspaceRepository::default(),
                &FakeActivityRepository,
                &job,
                &item,
                OutcomeClass::TransientFailure,
                Some("agent_crashed".into()),
                Some("agent crashed".into()),
                WorkspaceStatus::Ready,
            )
            .await;

            assert!(matches!(
                result,
                Err(UseCaseError::ProtocolViolation(message))
                    if message == "job failure does not match the current item revision"
            ));
        }

        #[tokio::test]
        async fn expire_job_maps_revision_stale_conflict() {
            let job = test_job(None);
            let item = nil_item();
            let job_repo = FakeJobRepository::with_finish_error(RepositoryError::Conflict(
                ConflictKind::JobRevisionStale,
            ));

            let result = expire_job(
                &job_repo,
                &FakeWorkspaceRepository::default(),
                &FakeActivityRepository,
                &job,
                &item,
                WorkspaceStatus::Ready,
            )
            .await;

            assert!(matches!(
                result,
                Err(UseCaseError::ProtocolViolation(message))
                    if message == "job expiration does not match the current item revision"
            ));
        }

        #[tokio::test]
        async fn cancel_job_releases_workspace_after_success() {
            let workspace = test_workspace();
            let job = test_job(Some(workspace.id));
            let item = nil_item();
            let workspace_repo = FakeWorkspaceRepository::with_workspace(workspace);

            let result = cancel_job(
                &FakeJobRepository::default(),
                &workspace_repo,
                &FakeActivityRepository,
                &job,
                &item,
                "operator_cancelled",
                WorkspaceStatus::Ready,
            )
            .await
            .expect("cancellation should succeed");

            let updated_workspace = workspace_repo.last_updated().expect("updated workspace");
            assert_eq!(result.released_workspace_id, Some(updated_workspace.id));
            assert_eq!(updated_workspace.state.current_job_id(), None);
            assert_eq!(updated_workspace.state.status(), WorkspaceStatus::Ready);
        }

        fn test_job(workspace_id: Option<WorkspaceId>) -> Job {
            let nil = Uuid::nil();
            let mut builder = JobBuilder::new(
                ProjectId::from_uuid(nil),
                ItemId::from_uuid(nil),
                ItemRevisionId::from_uuid(nil),
                "author_initial",
            )
            .id(JobId::from_uuid(nil))
            .status(JobStatus::Running);
            if let Some(workspace_id) = workspace_id {
                builder = builder.workspace_id(workspace_id);
            }
            builder.build()
        }

        fn test_workspace() -> Workspace {
            WorkspaceBuilder::new(ProjectId::from_uuid(Uuid::nil()), WorkspaceKind::Authoring)
                .id(WorkspaceId::from_uuid(Uuid::nil()))
                .status(WorkspaceStatus::Busy)
                .current_job_id(JobId::from_uuid(Uuid::nil()))
                .build()
        }

        #[derive(Clone, Default)]
        struct FakeJobRepository {
            finish_error: Arc<Mutex<Option<RepositoryError>>>,
        }

        impl FakeJobRepository {
            fn with_finish_error(error: RepositoryError) -> Self {
                Self {
                    finish_error: Arc::new(Mutex::new(Some(error))),
                }
            }
        }

        impl JobRepository for FakeJobRepository {
            async fn list_by_project(
                &self,
                _project_id: ProjectId,
            ) -> Result<Vec<Job>, RepositoryError> {
                async { unreachable!("unused in test") }.await
            }

            async fn list_by_revision(
                &self,
                _revision_id: ItemRevisionId,
            ) -> Result<Vec<Job>, RepositoryError> {
                async { unreachable!("unused in test") }.await
            }

            async fn get(&self, _id: JobId) -> Result<Job, RepositoryError> {
                async { unreachable!("unused in test") }.await
            }

            async fn create(&self, _job: &Job) -> Result<(), RepositoryError> {
                async { unreachable!("unused in test") }.await
            }

            async fn update(&self, _job: &Job) -> Result<(), RepositoryError> {
                async { unreachable!("unused in test") }.await
            }

            async fn find_active_for_revision(
                &self,
                _revision_id: ItemRevisionId,
            ) -> Result<Option<Job>, RepositoryError> {
                async { unreachable!("unused in test") }.await
            }

            async fn list_by_item(&self, _item_id: ItemId) -> Result<Vec<Job>, RepositoryError> {
                async { unreachable!("unused in test") }.await
            }

            async fn list_queued(&self, _limit: u32) -> Result<Vec<Job>, RepositoryError> {
                async { unreachable!("unused in test") }.await
            }

            async fn list_active(&self) -> Result<Vec<Job>, RepositoryError> {
                async { unreachable!("unused in test") }.await
            }

            async fn start_execution(
                &self,
                _params: StartJobExecutionParams,
            ) -> Result<(), RepositoryError> {
                async { unreachable!("unused in test") }.await
            }

            async fn heartbeat_execution(
                &self,
                _job_id: JobId,
                _item_id: ItemId,
                _revision_id: ItemRevisionId,
                _lease_owner_id: &ingot_domain::lease_owner_id::LeaseOwnerId,
                _lease_expires_at: chrono::DateTime<Utc>,
            ) -> Result<(), RepositoryError> {
                async { unreachable!("unused in test") }.await
            }

            async fn finish_non_success(
                &self,
                _params: FinishJobNonSuccessParams,
            ) -> Result<(), RepositoryError> {
                let finish_error = self.finish_error.clone();
                if let Some(error) = finish_error.lock().expect("finish error lock").take() {
                    return Err(error);
                }
                Ok(())
            }

            async fn delete(&self, _id: JobId) -> Result<(), RepositoryError> {
                async { unreachable!("unused in test") }.await
            }
        }

        #[derive(Clone, Default)]
        struct FakeWorkspaceRepository {
            state: Arc<Mutex<FakeWorkspaceRepositoryState>>,
        }

        #[derive(Default)]
        struct FakeWorkspaceRepositoryState {
            workspace: Option<Workspace>,
            updated: Option<Workspace>,
        }

        impl FakeWorkspaceRepository {
            fn with_workspace(workspace: Workspace) -> Self {
                Self {
                    state: Arc::new(Mutex::new(FakeWorkspaceRepositoryState {
                        workspace: Some(workspace),
                        updated: None,
                    })),
                }
            }

            fn last_updated(&self) -> Option<Workspace> {
                self.state
                    .lock()
                    .expect("workspace state lock")
                    .updated
                    .clone()
            }
        }

        impl WorkspaceRepository for FakeWorkspaceRepository {
            async fn list_by_project(
                &self,
                _project_id: ProjectId,
            ) -> Result<Vec<Workspace>, RepositoryError> {
                async { unreachable!("unused in test") }.await
            }

            async fn get(&self, _id: WorkspaceId) -> Result<Workspace, RepositoryError> {
                let workspace = self
                    .state
                    .lock()
                    .expect("workspace state lock")
                    .workspace
                    .clone();
                workspace.ok_or(RepositoryError::NotFound)
            }

            async fn create(&self, _workspace: &Workspace) -> Result<(), RepositoryError> {
                async { unreachable!("unused in test") }.await
            }

            async fn update(&self, workspace: &Workspace) -> Result<(), RepositoryError> {
                let state = self.state.clone();
                let workspace = workspace.clone();
                let mut state = state.lock().expect("workspace state lock");
                state.updated = Some(workspace.clone());
                state.workspace = Some(workspace);
                Ok(())
            }

            async fn find_authoring_for_revision(
                &self,
                _revision_id: ItemRevisionId,
            ) -> Result<Option<Workspace>, RepositoryError> {
                async { unreachable!("unused in test") }.await
            }

            async fn list_by_item(
                &self,
                _item_id: ItemId,
            ) -> Result<Vec<Workspace>, RepositoryError> {
                async { unreachable!("unused in test") }.await
            }

            async fn delete(&self, _id: WorkspaceId) -> Result<(), RepositoryError> {
                async { unreachable!("unused in test") }.await
            }
        }

        #[derive(Clone, Default)]
        struct FakeActivityRepository;

        impl ActivityRepository for FakeActivityRepository {
            async fn append(&self, _activity: &Activity) -> Result<(), RepositoryError> {
                Ok(())
            }

            async fn list_by_project(
                &self,
                _project_id: ProjectId,
                _limit: u32,
                _offset: u32,
            ) -> Result<Vec<Activity>, RepositoryError> {
                async { unreachable!("unused in test") }.await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use chrono::Utc;
    use ingot_domain::ids::{ItemId, ItemRevisionId, ProjectId};
    use ingot_domain::item::Escalation;
    use ingot_domain::job::{
        ContextPolicy, ExecutionPermission, JobInput, JobState, JobStatus, OutputArtifactKind,
        PhaseKind,
    };
    use ingot_domain::project::Project;
    use ingot_domain::test_support::{ConvergenceBuilder, JobBuilder, nil_item, nil_revision};
    use ingot_domain::workspace::WorkspaceKind;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;

    #[tokio::test]
    async fn completion_rejects_schema_mismatch_for_report_jobs() {
        let service = test_service(test_context(test_job(
            "validate_integrated",
            OutputArtifactKind::ValidationReport,
        )));

        let result = service
            .execute(CompleteJobCommand {
                job_id: JobId::from_uuid(Uuid::nil()),
                outcome_class: OutcomeClass::Clean,
                result_schema_version: Some("review_report:v1".into()),
                result_payload: Some(json!({
                    "outcome": "clean",
                    "summary": "ok",
                    "review_subject": {
                        "base_commit_oid": "base",
                        "head_commit_oid": "head"
                    },
                    "overall_risk": "low",
                    "findings": []
                })),
                output_commit_oid: None,
            })
            .await;

        assert!(matches!(
            result,
            Err(CompleteJobError::BadRequest {
                code: "invalid_result_schema_version",
                ..
            })
        ));
    }

    #[tokio::test]
    async fn completion_rejects_clean_validation_reports_with_failed_checks() {
        let service = test_service(test_context(test_job(
            "validate_candidate_initial",
            OutputArtifactKind::ValidationReport,
        )));

        let result = service
            .execute(CompleteJobCommand {
                job_id: JobId::from_uuid(Uuid::nil()),
                outcome_class: OutcomeClass::Clean,
                result_schema_version: Some("validation_report:v1".into()),
                result_payload: Some(json!({
                    "outcome": "clean",
                    "summary": "claimed clean despite a failed check",
                    "checks": [{
                        "name": "test",
                        "status": "fail",
                        "summary": "tests failed"
                    }],
                    "findings": [],
                    "extensions": null
                })),
                output_commit_oid: None,
            })
            .await;

        assert!(matches!(
            result,
            Err(CompleteJobError::UseCase(UseCaseError::ProtocolViolation(message)))
                if message.contains("failed checks")
        ));
    }

    #[tokio::test]
    async fn completion_rejects_report_payloads_with_unscoped_extra_fields() {
        let service = test_service(test_context(test_job(
            "validate_candidate_initial",
            OutputArtifactKind::ValidationReport,
        )));

        let result = service
            .execute(CompleteJobCommand {
                job_id: JobId::from_uuid(Uuid::nil()),
                outcome_class: OutcomeClass::Clean,
                result_schema_version: Some("validation_report:v1".into()),
                result_payload: Some(json!({
                    "outcome": "clean",
                    "summary": "ok",
                    "checks": [],
                    "findings": [],
                    "extensions": null,
                    "unexpected_provider_data": {
                        "must_live_under": "extensions"
                    }
                })),
                output_commit_oid: None,
            })
            .await;

        assert!(matches!(
            result,
            Err(CompleteJobError::UseCase(UseCaseError::ProtocolViolation(message)))
                if message.contains("unknown field") || message.contains("unexpected_provider_data")
        ));
    }

    #[tokio::test]
    async fn completion_supports_commit_jobs_without_report_payloads() {
        let mut job = test_job("repair_candidate", OutputArtifactKind::Commit);
        job.phase_kind = PhaseKind::Author;
        job.workspace_kind = WorkspaceKind::Authoring;
        job.execution_permission = ExecutionPermission::MayMutate;
        let repository = FakeRepository::new(test_context(job));
        let git = FakeGitPort::default().with_commit_exists(true);
        let service = CompleteJobService::new(repository.clone(), git, FakeProjectLocks::default());

        let result = service
            .execute(CompleteJobCommand {
                job_id: JobId::from_uuid(Uuid::nil()),
                outcome_class: OutcomeClass::Clean,
                result_schema_version: None,
                result_payload: None,
                output_commit_oid: Some("commit-oid".into()),
            })
            .await
            .expect("commit jobs should complete");

        let mutation = repository.last_mutation().expect("captured mutation");
        assert_eq!(result.finding_count, 0);
        assert_eq!(
            mutation.output_commit_oid.as_ref().map(CommitOid::as_str),
            Some("commit-oid")
        );
        assert!(mutation.result_schema_version.is_none());
    }

    #[tokio::test]
    async fn completion_sets_pending_approval_for_clean_integrated_validation() {
        let context = test_context(test_job(
            "validate_integrated",
            OutputArtifactKind::ValidationReport,
        ));
        // outcome_class is derived from the command, not the job state
        let repository = FakeRepository::new(context);
        let git = FakeGitPort::default();
        let service = CompleteJobService::new(repository.clone(), git, FakeProjectLocks::default());

        service
            .execute(valid_validation_command())
            .await
            .expect("clean integrated validation should complete");

        let mutation = repository.last_mutation().expect("captured mutation");
        let guard = mutation
            .prepared_convergence_guard
            .expect("prepared convergence guard");
        assert_eq!(guard.next_approval_state, Some(ApprovalState::Pending));
    }

    #[tokio::test]
    async fn completion_rejects_clean_integrated_validation_without_prepared_convergence() {
        let mut context = test_context(test_job(
            "validate_integrated",
            OutputArtifactKind::ValidationReport,
        ));
        // outcome_class is derived from the command, not the job state
        context.convergences.clear();
        let service = test_service(context);

        let result = service.execute(valid_validation_command()).await;

        assert!(matches!(
            result,
            Err(CompleteJobError::UseCase(
                UseCaseError::PreparedConvergenceMissing
            ))
        ));
    }

    #[tokio::test]
    async fn completion_rejects_clean_integrated_validation_when_target_ref_has_moved() {
        let context = test_context(test_job(
            "validate_integrated",
            OutputArtifactKind::ValidationReport,
        ));
        // outcome_class is derived from the command, not the job state
        let repository = FakeRepository::new(context);
        let git = FakeGitPort::default().with_hold_error(TargetRefHoldError::Stale);
        let service = CompleteJobService::new(repository, git, FakeProjectLocks::default());

        let result = service.execute(valid_validation_command()).await;

        assert!(matches!(
            result,
            Err(CompleteJobError::UseCase(
                UseCaseError::PreparedConvergenceStale
            ))
        ));
    }

    #[tokio::test]
    async fn completion_holds_target_ref_through_transaction_apply() {
        let context = test_context(test_job(
            "validate_integrated",
            OutputArtifactKind::ValidationReport,
        ));
        // outcome_class is derived from the command, not the job state
        let hold_active = Arc::new(AtomicBool::new(false));
        let hold_released = Arc::new(AtomicBool::new(false));
        let repository =
            FakeRepository::new(context).assert_hold_active_on_apply(hold_active.clone());
        let git =
            FakeGitPort::default().with_hold_state(hold_active.clone(), hold_released.clone());
        let service = CompleteJobService::new(repository, git, FakeProjectLocks::default());

        service
            .execute(valid_validation_command())
            .await
            .expect("job completion should succeed");

        assert!(
            hold_released.load(Ordering::SeqCst),
            "target ref hold should be released after apply"
        );
    }

    #[tokio::test]
    async fn completion_fails_when_target_ref_hold_release_fails_after_apply() {
        let context = test_context(test_job(
            "validate_integrated",
            OutputArtifactKind::ValidationReport,
        ));
        // outcome_class is derived from the command, not the job state
        let hold_active = Arc::new(AtomicBool::new(false));
        let hold_released = Arc::new(AtomicBool::new(false));
        let repository =
            FakeRepository::new(context).assert_hold_active_on_apply(hold_active.clone());
        let git = FakeGitPort::default()
            .with_hold_state(hold_active, hold_released.clone())
            .with_release_error(GitPortError::Internal("release timed out".into()));
        let service = CompleteJobService::new(repository.clone(), git, FakeProjectLocks::default());

        let result = service.execute(valid_validation_command()).await;

        assert!(matches!(
            result,
            Err(CompleteJobError::UseCase(UseCaseError::Internal(message)))
                if message == "git operation failed: release timed out"
        ));
        assert!(
            repository.last_mutation().is_some(),
            "completion mutation should still be applied"
        );
        assert!(
            !hold_released.load(Ordering::SeqCst),
            "target ref hold release should report failure"
        );
    }

    #[tokio::test]
    async fn completion_returns_apply_error_when_apply_and_release_hold_both_fail() {
        let context = test_context(test_job(
            "validate_integrated",
            OutputArtifactKind::ValidationReport,
        ));
        // outcome_class is derived from the command, not the job state
        let release_calls = Arc::new(AtomicUsize::new(0));
        let repository = FakeRepository::new(context)
            .with_apply_error(RepositoryError::Conflict(ConflictKind::JobRevisionStale));
        let git = FakeGitPort::default()
            .with_release_calls(release_calls.clone())
            .with_release_error(GitPortError::Internal("release timed out".into()));
        let service = CompleteJobService::new(repository, git, FakeProjectLocks::default());

        let result = service.execute(valid_validation_command()).await;

        assert!(matches!(
            result,
            Err(CompleteJobError::UseCase(UseCaseError::ProtocolViolation(message)))
                if message == "job completion does not match the current item revision"
        ));
        assert_eq!(
            release_calls.load(Ordering::SeqCst),
            1,
            "release should still be attempted when apply fails"
        );
    }

    #[tokio::test]
    async fn completion_retry_after_post_commit_hold_release_failure_returns_job_not_active() {
        let context = test_context(test_job(
            "validate_integrated",
            OutputArtifactKind::ValidationReport,
        ));
        // outcome_class is derived from the command, not the job state
        let repository = FakeRepository::new(context);
        let git = FakeGitPort::default()
            .with_release_error(GitPortError::Internal("release timed out".into()));
        let service = CompleteJobService::new(repository.clone(), git, FakeProjectLocks::default());

        let first_attempt = service.execute(valid_validation_command()).await;
        let retry = service.execute(valid_validation_command()).await;

        assert!(matches!(
            first_attempt,
            Err(CompleteJobError::UseCase(UseCaseError::Internal(message)))
                if message == "git operation failed: release timed out"
        ));
        assert!(matches!(
            retry,
            Err(CompleteJobError::UseCase(UseCaseError::JobNotActive))
        ));
        assert_eq!(
            repository.apply_count(),
            1,
            "hold-bearing retries should not reapply completion"
        );
    }

    #[tokio::test]
    async fn completion_returns_matching_completed_job_as_idempotent_success() {
        let mut job = test_job("investigate_item", OutputArtifactKind::FindingReport);
        job.phase_kind = PhaseKind::Investigate;
        job.workspace_kind = WorkspaceKind::Review;
        job.state = JobState::Completed {
            assignment: job.state.assignment().cloned(),
            started_at: job.state.started_at(),
            outcome_class: OutcomeClass::Findings,
            ended_at: Utc::now(),
            output_commit_oid: None,
            result_schema_version: Some("finding_report:v1".into()),
            result_payload: Some(json!({
                "outcome": "findings",
                "summary": "Found issues",
                "findings": [{
                    "finding_key": "f-1",
                    "code": "BUG001",
                    "severity": "high",
                    "summary": "first",
                    "paths": ["src/lib.rs"],
                    "evidence": ["broken"]
                }]
            })),
        };
        let repository = FakeRepository::new(test_context(job)).with_completion_finding_count(1);
        let service = CompleteJobService::new(
            repository,
            FakeGitPort::default(),
            FakeProjectLocks::default(),
        );

        let result = service
            .execute(completed_finding_report_command())
            .await
            .expect("matching completed job should be idempotent");

        assert_eq!(result.finding_count, 1);
    }

    #[tokio::test]
    async fn completion_rejects_mismatched_completed_job_retry() {
        let mut job = test_job("investigate_item", OutputArtifactKind::FindingReport);
        job.phase_kind = PhaseKind::Investigate;
        job.workspace_kind = WorkspaceKind::Review;
        job.state = JobState::Completed {
            assignment: job.state.assignment().cloned(),
            started_at: job.state.started_at(),
            outcome_class: OutcomeClass::Findings,
            ended_at: Utc::now(),
            output_commit_oid: None,
            result_schema_version: Some("finding_report:v1".into()),
            result_payload: Some(json!({
                "outcome": "findings",
                "summary": "Found issues",
                "findings": [{
                    "finding_key": "f-1",
                    "code": "BUG001",
                    "severity": "high",
                    "summary": "first",
                    "paths": ["src/lib.rs"],
                    "evidence": ["broken"]
                }]
            })),
        };
        let service = test_service(test_context(job));
        let mut mismatched_command = completed_finding_report_command();
        mismatched_command.result_payload = Some(json!({
            "outcome": "findings",
            "summary": "Changed summary",
            "findings": [{
                "finding_key": "f-1",
                "code": "BUG001",
                "severity": "high",
                "summary": "first",
                "paths": ["src/lib.rs"],
                "evidence": ["broken"]
            }]
        }));

        let result = service.execute(mismatched_command).await;

        assert!(matches!(
            result,
            Err(CompleteJobError::UseCase(UseCaseError::JobNotActive))
        ));
    }

    #[tokio::test]
    async fn completion_returns_job_not_active_for_malformed_inactive_job_requests() {
        let mut context = test_context(test_job(
            "validate_integrated",
            OutputArtifactKind::ValidationReport,
        ));
        context.job.state = ingot_domain::job::JobState::Terminated {
            terminal_status: ingot_domain::job::TerminalStatus::Failed,
            assignment: context.job.state.assignment().cloned(),
            started_at: context.job.state.started_at(),
            outcome_class: None,
            ended_at: Utc::now(),
            error_code: None,
            error_message: None,
        };
        let service = test_service(context);

        let result = service
            .execute(CompleteJobCommand {
                job_id: JobId::from_uuid(Uuid::nil()),
                outcome_class: OutcomeClass::Clean,
                result_schema_version: None,
                result_payload: None,
                output_commit_oid: None,
            })
            .await;

        assert!(matches!(
            result,
            Err(CompleteJobError::UseCase(UseCaseError::JobNotActive))
        ));
    }

    #[tokio::test]
    async fn completion_returns_job_not_active_for_malformed_completed_non_hold_retries() {
        let mut job = test_job("investigate_item", OutputArtifactKind::FindingReport);
        job.phase_kind = PhaseKind::Investigate;
        job.workspace_kind = WorkspaceKind::Review;
        job.state = JobState::Completed {
            assignment: job.state.assignment().cloned(),
            started_at: job.state.started_at(),
            outcome_class: OutcomeClass::Findings,
            ended_at: Utc::now(),
            output_commit_oid: None,
            result_schema_version: Some("finding_report:v1".into()),
            result_payload: Some(json!({
                "outcome": "findings",
                "summary": "Found issues",
                "findings": [{
                    "finding_key": "f-1",
                    "code": "BUG001",
                    "severity": "high",
                    "summary": "first",
                    "paths": ["src/lib.rs"],
                    "evidence": ["broken"]
                }]
            })),
        };
        let service = test_service(test_context(job));

        let result = service
            .execute(CompleteJobCommand {
                job_id: JobId::from_uuid(Uuid::nil()),
                outcome_class: OutcomeClass::Findings,
                result_schema_version: Some("finding_report:v1".into()),
                result_payload: None,
                output_commit_oid: None,
            })
            .await;

        assert!(matches!(
            result,
            Err(CompleteJobError::UseCase(UseCaseError::JobNotActive))
        ));
    }

    #[tokio::test]
    async fn completion_maps_transactional_revision_drift_to_protocol_violation() {
        let context = test_context(test_job(
            "validate_integrated",
            OutputArtifactKind::ValidationReport,
        ));
        let repository = FakeRepository::new(context)
            .with_apply_error(RepositoryError::Conflict(ConflictKind::JobRevisionStale));
        let service = CompleteJobService::new(
            repository,
            FakeGitPort::default(),
            FakeProjectLocks::default(),
        );

        let result = service.execute(valid_validation_command()).await;

        assert!(matches!(
            result,
            Err(CompleteJobError::UseCase(UseCaseError::ProtocolViolation(message)))
                if message == "job completion does not match the current item revision"
        ));
    }

    #[tokio::test]
    async fn completion_marks_successful_retry_to_clear_item_escalation() {
        let mut context = test_context(test_job(
            "validate_candidate_initial",
            OutputArtifactKind::ValidationReport,
        ));
        context.job.retry_no = 1;
        context.item.escalation = Escalation::OperatorRequired {
            reason: ingot_domain::item::EscalationReason::StepFailed,
        };
        let repository = FakeRepository::new(context);
        let service = CompleteJobService::new(
            repository.clone(),
            FakeGitPort::default(),
            FakeProjectLocks::default(),
        );

        service
            .execute(valid_validation_command())
            .await
            .expect("completion succeeds");

        let mutation = repository.last_mutation().expect("last mutation");
        assert!(mutation.clear_item_escalation);
    }

    #[tokio::test]
    async fn completion_does_not_clear_item_escalation_for_initial_success() {
        let mut context = test_context(test_job(
            "validate_candidate_initial",
            OutputArtifactKind::ValidationReport,
        ));
        context.item.escalation = Escalation::OperatorRequired {
            reason: ingot_domain::item::EscalationReason::StepFailed,
        };
        let repository = FakeRepository::new(context);
        let service = CompleteJobService::new(
            repository.clone(),
            FakeGitPort::default(),
            FakeProjectLocks::default(),
        );

        service
            .execute(valid_validation_command())
            .await
            .expect("completion succeeds");

        let mutation = repository.last_mutation().expect("last mutation");
        assert!(!mutation.clear_item_escalation);
    }

    fn valid_validation_command() -> CompleteJobCommand {
        CompleteJobCommand {
            job_id: JobId::from_uuid(Uuid::nil()),
            outcome_class: OutcomeClass::Clean,
            result_schema_version: Some("validation_report:v1".into()),
            result_payload: Some(json!({
                "outcome": "clean",
                "summary": "ok",
                "checks": [{
                    "name": "lint",
                    "status": "pass",
                    "summary": "ok"
                }],
                "findings": []
            })),
            output_commit_oid: None,
        }
    }

    fn completed_finding_report_command() -> CompleteJobCommand {
        CompleteJobCommand {
            job_id: JobId::from_uuid(Uuid::nil()),
            outcome_class: OutcomeClass::Findings,
            result_schema_version: Some("finding_report:v1".into()),
            result_payload: Some(json!({
                "outcome": "findings",
                "summary": "Found issues",
                "findings": [{
                    "finding_key": "f-1",
                    "code": "BUG001",
                    "severity": "high",
                    "summary": "first",
                    "paths": ["src/lib.rs"],
                    "evidence": ["broken"]
                }]
            })),
            output_commit_oid: None,
        }
    }

    fn test_service(
        context: JobCompletionContext,
    ) -> CompleteJobService<FakeRepository, FakeGitPort, FakeProjectLocks> {
        CompleteJobService::new(
            FakeRepository::new(context),
            FakeGitPort::default(),
            FakeProjectLocks::default(),
        )
    }

    fn test_context(job: Job) -> JobCompletionContext {
        JobCompletionContext {
            job,
            item: nil_item(),
            project: test_project(),
            revision: nil_revision(),
            convergences: vec![test_prepared_convergence()],
        }
    }

    fn test_project() -> Project {
        use ingot_domain::test_support::ProjectBuilder;
        use ingot_test_support::git::unique_temp_path;
        ProjectBuilder::new(unique_temp_path("ingot-usecases"))
            .id(ProjectId::from_uuid(Uuid::nil()))
            .name("Test")
            .build()
    }

    fn test_job(step_id: &str, output_artifact_kind: OutputArtifactKind) -> Job {
        let nil = Uuid::nil();
        JobBuilder::new(
            ProjectId::from_uuid(nil),
            ItemId::from_uuid(nil),
            ItemRevisionId::from_uuid(nil),
            step_id,
        )
        .id(JobId::from_uuid(nil))
        .status(JobStatus::Running)
        .outcome_class(OutcomeClass::Clean)
        .phase_kind(PhaseKind::Validate)
        .workspace_kind(WorkspaceKind::Integration)
        .execution_permission(ExecutionPermission::MustNotMutate)
        .context_policy(ContextPolicy::ResumeContext)
        .phase_template_slug("validate-integrated")
        .job_input(JobInput::integrated_subject(
            "target".into(),
            "prepared-head".into(),
        ))
        .output_artifact_kind(output_artifact_kind)
        .build()
    }

    fn test_prepared_convergence() -> ingot_domain::convergence::Convergence {
        ConvergenceBuilder::new(
            ProjectId::from_uuid(Uuid::nil()),
            ItemId::from_uuid(Uuid::nil()),
            ItemRevisionId::from_uuid(Uuid::nil()),
        )
        .id(ingot_domain::ids::ConvergenceId::from_uuid(Uuid::nil()))
        .source_head_commit_oid("prepared-head")
        .input_target_commit_oid("target")
        .prepared_commit_oid("prepared-head")
        .build()
    }

    #[derive(Clone)]
    struct FakeRepository {
        state: Arc<Mutex<FakeRepositoryState>>,
    }

    struct FakeRepositoryState {
        context: JobCompletionContext,
        last_mutation: Option<JobCompletionMutation>,
        apply_error: Option<RepositoryError>,
        hold_active: Option<Arc<AtomicBool>>,
        completion_finding_count: usize,
        apply_count: usize,
    }

    impl FakeRepository {
        fn new(context: JobCompletionContext) -> Self {
            Self {
                state: Arc::new(Mutex::new(FakeRepositoryState {
                    context,
                    last_mutation: None,
                    apply_error: None,
                    hold_active: None,
                    completion_finding_count: 0,
                    apply_count: 0,
                })),
            }
        }

        fn assert_hold_active_on_apply(self, hold_active: Arc<AtomicBool>) -> Self {
            self.state.lock().expect("state lock").hold_active = Some(hold_active);
            self
        }

        fn with_apply_error(self, apply_error: RepositoryError) -> Self {
            self.state.lock().expect("state lock").apply_error = Some(apply_error);
            self
        }

        fn with_completion_finding_count(self, completion_finding_count: usize) -> Self {
            self.state
                .lock()
                .expect("state lock")
                .completion_finding_count = completion_finding_count;
            self
        }

        fn last_mutation(&self) -> Option<JobCompletionMutation> {
            self.state.lock().expect("state lock").last_mutation.clone()
        }

        fn apply_count(&self) -> usize {
            self.state.lock().expect("state lock").apply_count
        }
    }

    impl JobCompletionRepository for FakeRepository {
        fn load_job_completion_context(
            &self,
            _job_id: JobId,
        ) -> impl std::future::Future<Output = Result<JobCompletionContext, RepositoryError>> + Send
        {
            let context = self.state.lock().expect("state lock").context.clone();
            async move { Ok(context) }
        }

        fn load_completed_job_completion(
            &self,
            _job_id: JobId,
        ) -> impl std::future::Future<
            Output = Result<Option<ingot_domain::ports::CompletedJobCompletion>, RepositoryError>,
        > + Send {
            let completed = {
                let state = self.state.lock().expect("state lock");
                (state.context.job.state.status() == JobStatus::Completed).then(|| {
                    ingot_domain::ports::CompletedJobCompletion {
                        job: state.context.job.clone(),
                        finding_count: state.completion_finding_count,
                    }
                })
            };
            async move { Ok(completed) }
        }

        fn apply_job_completion(
            &self,
            mutation: JobCompletionMutation,
        ) -> impl std::future::Future<Output = Result<(), RepositoryError>> + Send {
            let state = self.state.clone();
            async move {
                let mut state = state.lock().expect("state lock");
                state.apply_count += 1;
                if let Some(hold_active) = &state.hold_active {
                    assert!(
                        hold_active.load(Ordering::SeqCst),
                        "target ref hold should still be active during apply"
                    );
                }
                state.last_mutation = Some(mutation.clone());
                if let Some(error) = state.apply_error.take() {
                    return Err(error);
                }
                state.context.job.state = JobState::Completed {
                    assignment: state.context.job.state.assignment().cloned(),
                    started_at: state.context.job.state.started_at(),
                    outcome_class: mutation.outcome_class,
                    ended_at: chrono::Utc::now(),
                    output_commit_oid: mutation.output_commit_oid.clone(),
                    result_schema_version: mutation.result_schema_version.clone(),
                    result_payload: mutation.result_payload.clone(),
                };
                if mutation.clear_item_escalation {
                    state.context.item.escalation = ingot_domain::item::Escalation::None;
                }
                state.completion_finding_count = mutation.findings.len();
                Ok(())
            }
        }
    }

    #[derive(Clone, Default)]
    struct FakeGitPort {
        commit_exists: bool,
        hold_error: Option<Arc<Mutex<Option<TargetRefHoldError>>>>,
        hold_active: Option<Arc<AtomicBool>>,
        hold_released: Option<Arc<AtomicBool>>,
        release_error: Option<Arc<Mutex<Option<GitPortError>>>>,
        release_calls: Option<Arc<AtomicUsize>>,
    }

    #[derive(Debug)]
    struct FakeHold;

    impl FakeGitPort {
        fn with_commit_exists(mut self, commit_exists: bool) -> Self {
            self.commit_exists = commit_exists;
            self
        }

        fn with_hold_error(mut self, error: TargetRefHoldError) -> Self {
            self.hold_error = Some(Arc::new(Mutex::new(Some(error))));
            self
        }

        fn with_hold_state(
            mut self,
            hold_active: Arc<AtomicBool>,
            hold_released: Arc<AtomicBool>,
        ) -> Self {
            self.hold_active = Some(hold_active);
            self.hold_released = Some(hold_released);
            self
        }

        fn with_release_error(mut self, error: GitPortError) -> Self {
            self.release_error = Some(Arc::new(Mutex::new(Some(error))));
            self
        }

        fn with_release_calls(mut self, release_calls: Arc<AtomicUsize>) -> Self {
            self.release_calls = Some(release_calls);
            self
        }
    }

    impl JobCompletionGitPort for FakeGitPort {
        type Hold = FakeHold;

        fn commit_exists(
            &self,
            _repo_path: &Path,
            _commit_oid: &CommitOid,
        ) -> impl std::future::Future<Output = Result<bool, GitPortError>> + Send {
            let commit_exists = self.commit_exists;
            async move { Ok(commit_exists) }
        }

        fn verify_and_hold_target_ref(
            &self,
            _repo_path: &Path,
            _target_ref: &ingot_domain::git_ref::GitRef,
            _expected_oid: &CommitOid,
        ) -> impl std::future::Future<Output = Result<Self::Hold, TargetRefHoldError>> + Send
        {
            let hold_error = self.hold_error.clone();
            let hold_active = self.hold_active.clone();
            async move {
                if let Some(hold_error) = hold_error
                    && let Some(error) = hold_error.lock().expect("hold error lock").take()
                {
                    return Err(error);
                }

                if let Some(hold_active) = hold_active {
                    hold_active.store(true, Ordering::SeqCst);
                }

                Ok(FakeHold)
            }
        }

        fn release_hold(
            &self,
            _hold: Self::Hold,
        ) -> impl std::future::Future<Output = Result<(), GitPortError>> + Send {
            let hold_active = self.hold_active.clone();
            let hold_released = self.hold_released.clone();
            let release_error = self.release_error.clone();
            let release_calls = self.release_calls.clone();
            async move {
                if let Some(release_calls) = release_calls {
                    release_calls.fetch_add(1, Ordering::SeqCst);
                }
                if let Some(release_error) = release_error
                    && let Some(error) = release_error.lock().expect("release error lock").take()
                {
                    return Err(error);
                }

                if let Some(hold_active) = hold_active {
                    hold_active.store(false, Ordering::SeqCst);
                }
                if let Some(hold_released) = hold_released {
                    hold_released.store(true, Ordering::SeqCst);
                }
                Ok(())
            }
        }
    }

    #[derive(Clone, Default)]
    struct FakeProjectLocks {
        acquire_count: Arc<AtomicUsize>,
    }

    impl ProjectMutationLockPort for FakeProjectLocks {
        type Guard = ();

        fn acquire_project_mutation(
            &self,
            _project_id: ingot_domain::ids::ProjectId,
        ) -> impl std::future::Future<Output = Self::Guard> + Send {
            self.acquire_count.fetch_add(1, Ordering::SeqCst);
            async {}
        }
    }
}
