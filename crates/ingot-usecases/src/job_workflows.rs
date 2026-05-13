use std::future::Future;

use chrono::Utc;
use ingot_domain::activity::{Activity, ActivityEventType, ActivitySubject};
use ingot_domain::ids::{ActivityId, ProjectId};
use ingot_domain::item::ApprovalState;
use ingot_domain::job::{Job, OutcomeClass};
use ingot_domain::ports::{
    ActivityRepository, ItemRepository, JobCompletionGitPort, JobRepository,
    ProjectMutationLockPort, ProjectRepository, RepositoryError, WorkspaceRepository,
};

use crate::application::{ApplicationInfraPort, refresh_revision_context_for_job};
use crate::item_commands::auto_dispatch_projected_review_job;
use crate::store::{ApplicationJobContextStore, ItemRuntimeSnapshotStore};
use crate::{
    CompleteJobCommand, CompleteJobError, CompleteJobResult, CompleteJobService, UseCaseError,
};

pub trait CompleteJobExecutor: Send + Sync {
    fn execute(
        &self,
        command: CompleteJobCommand,
    ) -> impl Future<Output = Result<CompleteJobResult, CompleteJobError>> + Send;
}

#[derive(Debug)]
pub struct CompleteJobWorkflowOutput {
    pub finding_count: usize,
    pub auto_dispatch_result: Result<Option<Job>, UseCaseError>,
}

impl<R, G, L> CompleteJobExecutor for CompleteJobService<R, G, L>
where
    R: ingot_domain::ports::JobCompletionRepository,
    G: JobCompletionGitPort,
    L: ProjectMutationLockPort,
{
    async fn execute(
        &self,
        command: CompleteJobCommand,
    ) -> Result<CompleteJobResult, CompleteJobError> {
        CompleteJobService::execute(self, command).await
    }
}

pub async fn complete_job_workflow<R, I, C, L>(
    repo: &R,
    infra: &I,
    completion: &C,
    project_locks: &L,
    command: CompleteJobCommand,
) -> Result<CompleteJobWorkflowOutput, CompleteJobError>
where
    R: JobRepository
        + ItemRepository
        + ProjectRepository
        + WorkspaceRepository
        + ActivityRepository
        + ApplicationJobContextStore
        + ItemRuntimeSnapshotStore,
    I: ApplicationInfraPort,
    C: CompleteJobExecutor,
    L: ProjectMutationLockPort,
{
    let prior_job = <R as JobRepository>::get(repo, command.job_id)
        .await
        .map_err(map_repo_to_completion_error)?;
    let prior_item = <R as ItemRepository>::get(repo, prior_job.item_id)
        .await
        .map_err(map_item_get_to_completion_error)?;
    let project = <R as ProjectRepository>::get(repo, prior_job.project_id)
        .await
        .map_err(map_project_get_to_completion_error)?;
    infra
        .refresh_project_mirror(&project)
        .await
        .map_err(CompleteJobError::UseCase)?;

    let result = completion.execute(command).await?;
    refresh_revision_context_for_job(repo, infra, prior_job.id)
        .await
        .map_err(CompleteJobError::UseCase)?;

    let job = <R as JobRepository>::get(repo, prior_job.id)
        .await
        .map_err(map_repo_to_completion_error)?;
    let item = <R as ItemRepository>::get(repo, job.item_id)
        .await
        .map_err(map_item_get_to_completion_error)?;

    append_activity(
        repo,
        job.project_id,
        ActivityEventType::JobCompleted,
        ActivitySubject::Job(job.id),
        serde_json::json!({ "item_id": job.item_id, "outcome": job.state.outcome_class() }),
    )
    .await
    .map_err(CompleteJobError::UseCase)?;

    if prior_item.escalation.is_escalated()
        && item.current_revision_id == job.item_revision_id
        && !item.escalation.is_escalated()
    {
        append_activity(
            repo,
            job.project_id,
            ActivityEventType::ItemEscalationCleared,
            ActivitySubject::Item(item.id),
            serde_json::json!({ "reason": "successful_retry", "job_id": job.id }),
        )
        .await
        .map_err(CompleteJobError::UseCase)?;
    }

    if job.step_id == ingot_domain::step_id::StepId::ValidateIntegrated
        && job.state.outcome_class() == Some(OutcomeClass::Clean)
        && item.approval_state == ApprovalState::Pending
    {
        append_activity(
            repo,
            job.project_id,
            ActivityEventType::ApprovalRequested,
            ActivitySubject::Item(item.id),
            serde_json::json!({ "job_id": job.id }),
        )
        .await
        .map_err(CompleteJobError::UseCase)?;
    }

    let _guard = project_locks.acquire_project_mutation(project.id).await;
    let auto_dispatch_result =
        auto_dispatch_projected_review_job(repo, infra, &project, item.id).await;

    Ok(CompleteJobWorkflowOutput {
        finding_count: result.finding_count,
        auto_dispatch_result,
    })
}

fn map_repo_to_completion_error(error: RepositoryError) -> CompleteJobError {
    CompleteJobError::UseCase(UseCaseError::Repository(error))
}

fn map_project_get_to_completion_error(error: RepositoryError) -> CompleteJobError {
    match error {
        RepositoryError::NotFound => CompleteJobError::UseCase(UseCaseError::ProjectNotFound),
        other => map_repo_to_completion_error(other),
    }
}

fn map_item_get_to_completion_error(error: RepositoryError) -> CompleteJobError {
    match error {
        RepositoryError::NotFound => CompleteJobError::UseCase(UseCaseError::ItemNotFound),
        other => map_repo_to_completion_error(other),
    }
}

async fn append_activity<S>(
    activity_repo: &S,
    project_id: ProjectId,
    event_type: ActivityEventType,
    subject: ActivitySubject,
    payload: serde_json::Value,
) -> Result<(), UseCaseError>
where
    S: ActivityRepository,
{
    <S as ActivityRepository>::append(
        activity_repo,
        &Activity {
            id: ActivityId::new(),
            project_id,
            event_type,
            subject,
            payload,
            created_at: Utc::now(),
        },
    )
    .await?;
    Ok(())
}
