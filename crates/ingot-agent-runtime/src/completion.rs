// Shared post-report completion orchestration.

use std::future::Future;

use ingot_domain::activity::{ActivityEventType, ActivitySubject};
use ingot_domain::commit_oid::CommitOid;
use ingot_domain::ids::{ItemId, JobId, ProjectId};
use ingot_domain::item::ApprovalState;
use ingot_domain::job::OutcomeClass;
use ingot_domain::ports::RepositoryError;
use ingot_domain::step_id::StepId;
use ingot_usecases::{CompleteJobCommand, CompleteJobError};

use crate::{JobDispatcher, RuntimeError, outcome_class_name};

pub(crate) struct ReportCompletion {
    pub(crate) job_id: JobId,
    pub(crate) item_id: ItemId,
    pub(crate) project_id: ProjectId,
    pub(crate) step_id: StepId,
    pub(crate) outcome_class: OutcomeClass,
    pub(crate) result_schema_version: String,
    pub(crate) result_payload: serde_json::Value,
    pub(crate) output_commit_oid: Option<CommitOid>,
}

#[derive(Debug)]
pub(crate) enum ReportCompletionError {
    CompletionRejected(CompleteJobError),
    Runtime(RuntimeError),
}

impl From<RuntimeError> for ReportCompletionError {
    fn from(error: RuntimeError) -> Self {
        Self::Runtime(error)
    }
}

impl From<RepositoryError> for ReportCompletionError {
    fn from(error: RepositoryError) -> Self {
        Self::Runtime(RuntimeError::Repository(error))
    }
}

impl JobDispatcher {
    pub(crate) async fn complete_report_with_finalizers<F, Fut, G, Gut, H, Hut>(
        &self,
        completion: ReportCompletion,
        finalize_workspace: F,
        refresh_context: G,
        after_refresh: H,
    ) -> Result<(), ReportCompletionError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(), RuntimeError>>,
        G: FnOnce() -> Gut,
        Gut: Future<Output = Result<(), RuntimeError>>,
        H: FnOnce() -> Hut,
        Hut: Future<Output = Result<(), RuntimeError>>,
    {
        self.complete_job_service()
            .execute(CompleteJobCommand {
                job_id: completion.job_id,
                outcome_class: completion.outcome_class,
                result_schema_version: Some(completion.result_schema_version),
                result_payload: Some(completion.result_payload),
                output_commit_oid: completion.output_commit_oid,
            })
            .await
            .map_err(ReportCompletionError::CompletionRejected)?;

        self.append_activity(
            completion.project_id,
            ActivityEventType::JobCompleted,
            ActivitySubject::Job(completion.job_id),
            serde_json::json!({
                "item_id": completion.item_id,
                "outcome": outcome_class_name(completion.outcome_class),
            }),
        )
        .await?;

        if completion.step_id == StepId::ValidateIntegrated
            && completion.outcome_class == OutcomeClass::Clean
        {
            let updated_item = self.db.get_item(completion.item_id).await?;
            if updated_item.approval_state == ApprovalState::Pending {
                self.append_activity(
                    completion.project_id,
                    ActivityEventType::ApprovalRequested,
                    ActivitySubject::Item(completion.item_id),
                    serde_json::json!({ "job_id": completion.job_id }),
                )
                .await?;
            }
        }

        if completion.outcome_class == OutcomeClass::Findings {
            let project = self.db.get_project(completion.project_id).await?;
            if project.execution_mode == ingot_domain::project::ExecutionMode::Autopilot {
                let item = self.db.get_item(completion.item_id).await?;
                self.auto_triage_job_findings(&project, completion.job_id, &item)
                    .await?;
            }
        }

        finalize_workspace().await?;
        refresh_context().await?;
        after_refresh().await?;
        self.auto_dispatch_projected_review(completion.project_id, completion.item_id)
            .await?;

        Ok(())
    }
}
