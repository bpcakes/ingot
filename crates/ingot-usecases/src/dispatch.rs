use std::future::Future;

mod planning {
    use ingot_domain::convergence::Convergence;
    use ingot_domain::finding::Finding;
    use ingot_domain::ids::JobId;
    use ingot_domain::item::{Item, ParkingState};
    use ingot_domain::job::{Job, JobInput, JobState, OutcomeClass};
    use ingot_domain::revision::ItemRevision;
    use ingot_domain::step_id::StepId;
    use ingot_workflow::{Evaluation, Evaluator, StepContract, step};

    use crate::UseCaseError;
    use crate::authoring_history::{
        current_authoring_head_for_revision, job_input_from_prepared_convergence,
        previous_authoring_head_for_revision, selected_prepared_convergence,
        subject_input_from_range,
    };

    #[derive(Debug, Clone)]
    pub struct DispatchJobCommand {
        pub step_id: Option<StepId>,
    }

    pub fn dispatch_job(
        item: &Item,
        revision: &ItemRevision,
        jobs: &[Job],
        findings: &[Finding],
        convergences: &[Convergence],
        command: DispatchJobCommand,
    ) -> Result<Job, UseCaseError> {
        ensure_item_dispatchable(item)?;
        ensure_no_active_jobs(item.current_revision_id, jobs)?;

        let evaluation = Evaluator::new().evaluate(item, revision, jobs, findings, convergences);
        let step_id = select_dispatch_step(&evaluation, command.step_id)?;
        ensure_no_conflicting_convergence(step_id, item.current_revision_id, convergences)?;
        let contract = step::find_step(step_id);

        if !contract.is_dispatchable_job() {
            return Err(UseCaseError::IllegalStepDispatch(format!(
                "Step is not dispatchable: {step_id}"
            )));
        }

        let template_slug =
            template_slug_for_step(revision, step_id, contract.default_template_slug);
        let job_input = job_input_for_step(step_id, revision, jobs, convergences);
        let semantic_attempt_no = next_semantic_attempt_no(jobs, item.current_revision_id, step_id);

        let job_spec = QueuedJobSpec {
            step_id,
            semantic_attempt_no,
            retry_no: 0,
            supersedes_job_id: None,
            phase_template_slug: template_slug,
            job_input,
        };

        Ok(queued_job(item, contract, job_spec))
    }

    pub fn retry_job(
        item: &Item,
        revision: &ItemRevision,
        jobs: &[Job],
        findings: &[Finding],
        convergences: &[Convergence],
        previous_job: &Job,
    ) -> Result<Job, UseCaseError> {
        ensure_item_dispatchable(item)?;

        if previous_job.item_revision_id != item.current_revision_id {
            return Err(UseCaseError::IllegalStepDispatch(
                "Cannot retry a job from a superseded revision".into(),
            ));
        }

        ensure_no_active_jobs(item.current_revision_id, jobs)?;

        if !previous_job.state.is_terminal()
            || matches!(
                previous_job.state.outcome_class(),
                Some(OutcomeClass::Clean | OutcomeClass::Findings)
            )
        {
            return Err(UseCaseError::IllegalStepDispatch(
                "Only terminal non-success jobs can be retried".into(),
            ));
        }

        let evaluation = Evaluator::new().evaluate(item, revision, jobs, findings, convergences);
        let contract = step::find_step(previous_job.step_id);

        if contract.execution_permission == ingot_domain::job::ExecutionPermission::DaemonOnly {
            return Err(UseCaseError::IllegalStepDispatch(
                "Daemon-executed jobs cannot be retried manually".into(),
            ));
        }

        let closure_position_allows_retry =
            evaluation.current_step_id == Some(previous_job.step_id);
        let report_only_retry = evaluation
            .auxiliary_dispatchable_step_ids
            .iter()
            .any(|step_id| step_id == &previous_job.step_id);

        if !closure_position_allows_retry && !report_only_retry {
            return Err(UseCaseError::IllegalStepDispatch(format!(
                "Step is not retryable in the current state: {}",
                previous_job.step_id
            )));
        }
        ensure_no_conflicting_convergence(
            previous_job.step_id,
            item.current_revision_id,
            convergences,
        )?;

        let template_slug = template_slug_for_step(
            revision,
            previous_job.step_id,
            contract.default_template_slug,
        );
        let job_input = job_input_for_step(previous_job.step_id, revision, jobs, convergences);
        let retry_no = jobs
            .iter()
            .filter(|job| job.item_revision_id == item.current_revision_id)
            .filter(|job| job.step_id == previous_job.step_id)
            .filter(|job| job.semantic_attempt_no == previous_job.semantic_attempt_no)
            .map(|job| job.retry_no)
            .max()
            .unwrap_or(previous_job.retry_no)
            + 1;

        let job_spec = QueuedJobSpec {
            step_id: previous_job.step_id,
            semantic_attempt_no: previous_job.semantic_attempt_no,
            retry_no,
            supersedes_job_id: Some(previous_job.id),
            phase_template_slug: template_slug,
            job_input,
        };

        Ok(queued_job(item, contract, job_spec))
    }

    struct QueuedJobSpec {
        step_id: StepId,
        semantic_attempt_no: u32,
        retry_no: u32,
        supersedes_job_id: Option<JobId>,
        phase_template_slug: String,
        job_input: JobInput,
    }

    fn queued_job(item: &Item, contract: &StepContract, spec: QueuedJobSpec) -> Job {
        Job {
            id: JobId::new(),
            project_id: item.project_id,
            item_id: item.id,
            item_revision_id: item.current_revision_id,
            step_id: spec.step_id,
            semantic_attempt_no: spec.semantic_attempt_no,
            retry_no: spec.retry_no,
            supersedes_job_id: spec.supersedes_job_id,
            phase_kind: contract.phase_kind,
            workspace_kind: contract.workspace_kind,
            execution_permission: contract.execution_permission,
            context_policy: contract.context_policy,
            phase_template_slug: spec.phase_template_slug,
            job_input: spec.job_input,
            output_artifact_kind: contract.output_artifact_kind,
            created_at: chrono::Utc::now(),
            state: JobState::Queued,
        }
    }

    fn select_dispatch_step(
        evaluation: &Evaluation,
        requested_step_id: Option<StepId>,
    ) -> Result<StepId, UseCaseError> {
        if let Some(requested_step_id) = requested_step_id {
            if evaluation.dispatchable_step_id == Some(requested_step_id)
                || evaluation
                    .auxiliary_dispatchable_step_ids
                    .contains(&requested_step_id)
            {
                return Ok(requested_step_id);
            }

            return Err(UseCaseError::IllegalStepDispatch(format!(
                "Step is not dispatchable in the current state: {requested_step_id}"
            )));
        }

        evaluation.dispatchable_step_id.ok_or_else(|| {
            UseCaseError::IllegalStepDispatch(
                "No closure-relevant step is dispatchable in the current state".into(),
            )
        })
    }

    fn template_slug_for_step(
        revision: &ItemRevision,
        step_id: StepId,
        default_template_slug: Option<&'static str>,
    ) -> String {
        revision
            .template_map_snapshot
            .get(step_id.as_str())
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned)
            .or_else(|| default_template_slug.map(ToOwned::to_owned))
            .unwrap_or_else(|| step_id.to_string())
    }

    fn job_input_for_step(
        step_id: StepId,
        revision: &ItemRevision,
        jobs: &[Job],
        convergences: &[Convergence],
    ) -> JobInput {
        let seed_head = revision.seed.seed_commit_oid().map(ToOwned::to_owned);
        let current_head = current_authoring_head_for_revision(jobs, revision);
        let previous_head = previous_authoring_head_for_revision(jobs, revision);
        let prepared_convergence = selected_prepared_convergence(revision.id, convergences);

        match step_id {
            StepId::AuthorInitial => seed_head
                .map(JobInput::authoring_head)
                .unwrap_or(JobInput::None),
            StepId::RepairCandidate | StepId::RepairAfterIntegration => current_head
                .map(JobInput::authoring_head)
                .unwrap_or(JobInput::None),
            StepId::ReviewIncrementalInitial => {
                subject_input_from_range(seed_head, current_head, false)
            }
            StepId::ReviewIncrementalRepair | StepId::ReviewIncrementalAfterIntegrationRepair => {
                subject_input_from_range(previous_head, current_head, false)
            }
            StepId::ReviewCandidateInitial
            | StepId::ReviewCandidateRepair
            | StepId::ValidateCandidateInitial
            | StepId::ValidateCandidateRepair
            | StepId::ReviewAfterIntegrationRepair
            | StepId::ValidateAfterIntegrationRepair => {
                subject_input_from_range(seed_head, current_head, false)
            }
            StepId::InvestigateItem => prepared_convergence
                .map(|convergence| job_input_from_prepared_convergence(convergence, false))
                .unwrap_or_else(|| subject_input_from_range(seed_head, current_head, false)),
            StepId::InvestigateProject | StepId::ReinvestigateProject => seed_head
                .map(JobInput::authoring_head)
                .unwrap_or(JobInput::None),
            StepId::ValidateIntegrated => prepared_convergence
                .map(|convergence| job_input_from_prepared_convergence(convergence, true))
                .unwrap_or(JobInput::None),
            _ => JobInput::None,
        }
    }

    fn ensure_item_dispatchable(item: &Item) -> Result<(), UseCaseError> {
        if !item.lifecycle.is_open() {
            return Err(UseCaseError::ItemNotOpen);
        }

        if item.parking_state != ParkingState::Active {
            return Err(UseCaseError::ItemNotIdle);
        }

        Ok(())
    }

    fn ensure_no_active_jobs(
        revision_id: ingot_domain::ids::ItemRevisionId,
        jobs: &[Job],
    ) -> Result<(), UseCaseError> {
        if jobs
            .iter()
            .any(|job| job.item_revision_id == revision_id && job.state.is_active())
        {
            return Err(UseCaseError::ActiveJobExists);
        }

        Ok(())
    }

    fn ensure_no_conflicting_convergence(
        step_id: StepId,
        revision_id: ingot_domain::ids::ItemRevisionId,
        convergences: &[Convergence],
    ) -> Result<(), UseCaseError> {
        if convergences.iter().any(|convergence| {
            convergence.item_revision_id == revision_id
                && match convergence.state.status() {
                    ingot_domain::convergence::ConvergenceStatus::Queued
                    | ingot_domain::convergence::ConvergenceStatus::Running => true,
                    ingot_domain::convergence::ConvergenceStatus::Prepared => !matches!(
                        step_id,
                        StepId::ValidateIntegrated | StepId::InvestigateItem
                    ),
                    ingot_domain::convergence::ConvergenceStatus::Conflicted
                    | ingot_domain::convergence::ConvergenceStatus::Finalized
                    | ingot_domain::convergence::ConvergenceStatus::Failed
                    | ingot_domain::convergence::ConvergenceStatus::Cancelled => false,
                }
        }) {
            return Err(UseCaseError::ActiveConvergenceExists);
        }

        Ok(())
    }

    fn next_semantic_attempt_no(
        jobs: &[Job],
        revision_id: ingot_domain::ids::ItemRevisionId,
        step_id: StepId,
    ) -> u32 {
        jobs.iter()
            .filter(|job| job.item_revision_id == revision_id && job.step_id == step_id)
            .map(|job| job.semantic_attempt_no)
            .max()
            .unwrap_or(0)
            + 1
    }

    #[cfg(test)]
    mod tests {
        use chrono::Utc;
        use ingot_domain::commit_oid::CommitOid;
        use ingot_domain::ids::{ItemId, ItemRevisionId, JobId, ProjectId};
        use ingot_domain::job::{
            ContextPolicy, ExecutionPermission, JobInput, JobState, JobStatus, OutcomeClass,
            OutputArtifactKind, PhaseKind,
        };
        use ingot_domain::test_support::{ConvergenceBuilder, JobBuilder, nil_item, nil_revision};
        use ingot_domain::workspace::WorkspaceKind;
        use serde_json::json;
        use uuid::Uuid;

        use super::*;

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

        #[test]
        fn dispatch_after_repair_commit_reenters_incremental_review_before_candidate_review() {
            let item = nil_item();
            let revision = nil_revision();

            let mut author_initial = test_job("author_initial", OutputArtifactKind::Commit);
            author_initial.phase_kind = PhaseKind::Author;
            author_initial.workspace_kind = WorkspaceKind::Authoring;
            author_initial.execution_permission = ExecutionPermission::MayMutate;
            author_initial.state = JobState::Completed {
                assignment: author_initial.state.assignment().cloned(),
                started_at: author_initial.state.started_at(),
                outcome_class: OutcomeClass::Clean,
                ended_at: Utc::now(),
                output_commit_oid: Some("commit-1".into()),
                result_schema_version: None,
                result_payload: None,
            };

            let mut review_incremental = test_job(
                "review_incremental_initial",
                OutputArtifactKind::ReviewReport,
            );
            review_incremental.id = JobId::from_uuid(Uuid::now_v7());
            review_incremental.phase_kind = PhaseKind::Review;
            review_incremental.workspace_kind = WorkspaceKind::Review;
            review_incremental.execution_permission = ExecutionPermission::MustNotMutate;
            review_incremental.state = JobState::Completed {
                assignment: review_incremental.state.assignment().cloned(),
                started_at: review_incremental.state.started_at(),
                outcome_class: OutcomeClass::Findings,
                ended_at: Utc::now(),
                output_commit_oid: None,
                result_schema_version: Some("review_report:v1".into()),
                result_payload: Some(json!({
                    "outcome": "findings",
                    "summary": "needs repair",
                    "review_subject": {
                        "base_commit_oid": "seed",
                        "head_commit_oid": "commit-1"
                    },
                    "overall_risk": "medium",
                    "findings": [{
                      "finding_key": "f1",
                      "code": "BUG",
                      "severity": "medium",
                      "summary": "repair",
                      "paths": ["src/lib.rs"],
                      "evidence": ["repair"]
                    }]
                })),
            };

            let mut repair_candidate = test_job("repair_candidate", OutputArtifactKind::Commit);
            repair_candidate.id = JobId::from_uuid(Uuid::now_v7());
            repair_candidate.phase_kind = PhaseKind::Author;
            repair_candidate.workspace_kind = WorkspaceKind::Authoring;
            repair_candidate.execution_permission = ExecutionPermission::MayMutate;
            repair_candidate.state = JobState::Completed {
                assignment: repair_candidate.state.assignment().cloned(),
                started_at: repair_candidate.state.started_at(),
                outcome_class: OutcomeClass::Clean,
                ended_at: Utc::now(),
                output_commit_oid: Some("commit-2".into()),
                result_schema_version: None,
                result_payload: None,
            };

            let job = dispatch_job(
                &item,
                &revision,
                &[author_initial, review_incremental, repair_candidate],
                &[],
                &[],
                DispatchJobCommand { step_id: None },
            )
            .expect("dispatch after repair");

            assert_eq!(job.step_id, StepId::ReviewIncrementalRepair);
            assert_eq!(
                job.job_input.base_commit_oid().map(CommitOid::as_str),
                Some("commit-1")
            );
            assert_eq!(
                job.job_input.head_commit_oid().map(CommitOid::as_str),
                Some("commit-2")
            );
        }

        #[test]
        fn dispatch_after_clean_incremental_repair_advances_to_candidate_review_then_validation() {
            let item = nil_item();
            let revision = nil_revision();

            let mut repair_candidate = test_job("repair_candidate", OutputArtifactKind::Commit);
            repair_candidate.phase_kind = PhaseKind::Author;
            repair_candidate.workspace_kind = WorkspaceKind::Authoring;
            repair_candidate.execution_permission = ExecutionPermission::MayMutate;
            repair_candidate.state = JobState::Completed {
                assignment: repair_candidate.state.assignment().cloned(),
                started_at: repair_candidate.state.started_at(),
                outcome_class: OutcomeClass::Clean,
                ended_at: Utc::now(),
                output_commit_oid: Some("commit-2".into()),
                result_schema_version: None,
                result_payload: None,
            };

            let mut review_incremental_repair = test_job(
                "review_incremental_repair",
                OutputArtifactKind::ReviewReport,
            );
            review_incremental_repair.id = JobId::from_uuid(Uuid::now_v7());
            review_incremental_repair.phase_kind = PhaseKind::Review;
            review_incremental_repair.workspace_kind = WorkspaceKind::Review;
            review_incremental_repair.execution_permission = ExecutionPermission::MustNotMutate;
            review_incremental_repair.state = JobState::Completed {
                assignment: review_incremental_repair.state.assignment().cloned(),
                started_at: review_incremental_repair.state.started_at(),
                outcome_class: OutcomeClass::Clean,
                ended_at: Utc::now(),
                output_commit_oid: None,
                result_schema_version: Some("review_report:v1".into()),
                result_payload: Some(json!({
                    "outcome": "clean",
                    "summary": "incremental clean",
                    "review_subject": {
                        "base_commit_oid": "seed",
                        "head_commit_oid": "commit-2"
                    },
                    "overall_risk": "low",
                    "findings": []
                })),
            };

            let candidate_review_job = dispatch_job(
                &item,
                &revision,
                &[repair_candidate.clone(), review_incremental_repair.clone()],
                &[],
                &[],
                DispatchJobCommand { step_id: None },
            )
            .expect("dispatch candidate review");
            assert_eq!(candidate_review_job.step_id, StepId::ReviewCandidateRepair);

            let mut review_candidate_repair =
                test_job("review_candidate_repair", OutputArtifactKind::ReviewReport);
            review_candidate_repair.id = JobId::from_uuid(Uuid::now_v7());
            review_candidate_repair.phase_kind = PhaseKind::Review;
            review_candidate_repair.workspace_kind = WorkspaceKind::Review;
            review_candidate_repair.execution_permission = ExecutionPermission::MustNotMutate;
            review_candidate_repair.state = JobState::Completed {
                assignment: review_candidate_repair.state.assignment().cloned(),
                started_at: review_candidate_repair.state.started_at(),
                outcome_class: OutcomeClass::Clean,
                ended_at: Utc::now(),
                output_commit_oid: None,
                result_schema_version: Some("review_report:v1".into()),
                result_payload: Some(json!({
                    "outcome": "clean",
                    "summary": "candidate clean",
                    "review_subject": {
                        "base_commit_oid": "seed",
                        "head_commit_oid": "commit-2"
                    },
                    "overall_risk": "low",
                    "findings": []
                })),
            };

            let validation_job = dispatch_job(
                &item,
                &revision,
                &[
                    repair_candidate,
                    review_incremental_repair,
                    review_candidate_repair,
                ],
                &[],
                &[],
                DispatchJobCommand { step_id: None },
            )
            .expect("dispatch validation");
            assert_eq!(validation_job.step_id, StepId::ValidateCandidateRepair);
            assert_eq!(
                validation_job
                    .job_input
                    .base_commit_oid()
                    .map(CommitOid::as_str),
                Some("seed")
            );
            assert_eq!(
                validation_job
                    .job_input
                    .head_commit_oid()
                    .map(CommitOid::as_str),
                Some("commit-2")
            );
        }

        #[test]
        fn dispatch_allows_integrated_validation_while_prepared_convergence_exists() {
            let item = nil_item();
            let revision = nil_revision();

            let mut validation_candidate = test_job(
                "validate_candidate_initial",
                OutputArtifactKind::ValidationReport,
            );
            validation_candidate.phase_kind = PhaseKind::Validate;
            validation_candidate.workspace_kind = WorkspaceKind::Authoring;
            validation_candidate.execution_permission = ExecutionPermission::DaemonOnly;
            validation_candidate.job_input =
                JobInput::candidate_subject("seed".into(), "candidate-head".into());
            validation_candidate.state = JobState::Completed {
                assignment: validation_candidate.state.assignment().cloned(),
                started_at: validation_candidate.state.started_at(),
                outcome_class: OutcomeClass::Clean,
                ended_at: Utc::now(),
                output_commit_oid: None,
                result_schema_version: None,
                result_payload: None,
            };

            let convergence = ConvergenceBuilder::new(item.project_id, item.id, revision.id)
                .input_target_commit_oid("target-head")
                .prepared_commit_oid("prepared-head")
                .build();

            let job = dispatch_job(
                &item,
                &revision,
                &[validation_candidate],
                &[],
                &[convergence],
                DispatchJobCommand { step_id: None },
            )
            .expect("dispatch integrated validation");

            assert_eq!(job.step_id, StepId::ValidateIntegrated);
            assert_eq!(
                job.job_input.base_commit_oid().map(CommitOid::as_str),
                Some("target-head")
            );
            assert_eq!(
                job.job_input.head_commit_oid().map(CommitOid::as_str),
                Some("prepared-head")
            );
        }
    }
}

use chrono::Utc;
use ingot_domain::activity::{Activity, ActivityEventType, ActivitySubject};
use ingot_domain::commit_oid::CommitOid;
use ingot_domain::convergence::Convergence;
use ingot_domain::finding::Finding;
use ingot_domain::git_operation::{
    GitOperation, GitOperationEntityRef, GitOperationStatus, OperationPayload,
};
use ingot_domain::git_ref::GitRef;
use ingot_domain::ids::{ActivityId, GitOperationId, ItemId, ItemRevisionId, JobId, ProjectId};
use ingot_domain::item::{EscalationReason, Item};
use ingot_domain::job::{ExecutionPermission, Job, JobInput, JobStatus, OutcomeClass};
use ingot_domain::ports::{
    ActivityRepository, FindingRepository, GitOperationRepository, JobRepository,
    WorkspaceRepository,
};
use ingot_domain::project::Project;
use ingot_domain::revision::ItemRevision;
use ingot_domain::step_id::StepId;
use ingot_domain::workspace::{Workspace, WorkspaceKind};
use ingot_workflow::{ClosureRelevance, Evaluator, step};

use crate::UseCaseError;
use crate::authoring_history::build_candidate_subject_input;
use crate::git_operation_journal::{create_planned, mark_applied};
use crate::store::{AutoDispatchStore, DispatchStore};

pub use planning::{DispatchJobCommand, dispatch_job, retry_job};

pub trait DispatchInfraPort: Send + Sync {
    fn resolve_ref_oid(
        &self,
        project_id: ProjectId,
        ref_name: &GitRef,
    ) -> impl Future<Output = Result<Option<CommitOid>, UseCaseError>> + Send;

    fn update_ref(
        &self,
        project_id: ProjectId,
        ref_name: &GitRef,
        commit_oid: &CommitOid,
    ) -> impl Future<Output = Result<(), UseCaseError>> + Send;

    fn delete_ref(
        &self,
        project_id: ProjectId,
        ref_name: &GitRef,
    ) -> impl Future<Output = Result<(), UseCaseError>> + Send;

    fn remove_workspace_files(
        &self,
        project_id: ProjectId,
        workspace: &Workspace,
    ) -> impl Future<Output = Result<(), UseCaseError>> + Send;

    fn ensure_authoring_workspace(
        &self,
        project_id: ProjectId,
        revision: &ItemRevision,
        job: &Job,
        existing: Option<Workspace>,
    ) -> impl Future<Output = Result<Workspace, UseCaseError>> + Send;
}

#[must_use]
pub fn investigation_ref_name(job_id: JobId) -> GitRef {
    GitRef::new(format!("refs/ingot/investigations/{job_id}"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingInvestigationRef {
    pub ref_name: GitRef,
    pub commit_oid: CommitOid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchActivityContext {
    pub dispatch_origin: Option<&'static str>,
    pub supersedes_job_id: Option<JobId>,
    pub retry_no: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct PreparedDispatchedJob {
    pub job: Job,
}

struct EnsuredAuthoringWorkspace {
    workspace: Workspace,
    created: bool,
}

pub async fn plan_and_apply_investigation_ref<S, G>(
    store: &S,
    git_port: &G,
    project_id: ProjectId,
    entity: GitOperationEntityRef,
    ref_name: &GitRef,
    commit_oid: &CommitOid,
) -> Result<(), UseCaseError>
where
    S: GitOperationRepository + ActivityRepository,
    G: DispatchInfraPort,
{
    let mut operation = GitOperation {
        id: GitOperationId::new(),
        project_id,
        entity,
        payload: OperationPayload::CreateInvestigationRef {
            ref_name: ref_name.clone(),
            new_oid: commit_oid.clone(),
            commit_oid: Some(commit_oid.clone()),
        },
        status: GitOperationStatus::Planned,
        created_at: Utc::now(),
        completed_at: None,
    };
    create_planned(store, store, &operation, project_id).await?;
    git_port
        .update_ref(project_id, ref_name, commit_oid)
        .await?;
    mark_applied(store, &mut operation).await?;
    Ok(())
}

pub async fn cleanup_failed_dispatch<S, G>(
    store: &S,
    git_port: &G,
    project_id: ProjectId,
    precreated_workspace: Option<&Workspace>,
    investigation_ref_name: Option<&GitRef>,
) where
    S: WorkspaceRepository + GitOperationRepository,
    G: DispatchInfraPort,
{
    if let Some(workspace) = precreated_workspace {
        let _ = git_port.remove_workspace_files(project_id, workspace).await;
        let _ = store.delete(workspace.id).await;
    }

    if let Some(ref_name) = investigation_ref_name {
        let _ = git_port.delete_ref(project_id, ref_name).await;
        let _ = store.delete_investigation_ref_operations(ref_name).await;
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn apply_pending_investigation_ref_or_cleanup<S, G>(
    store: &S,
    git_port: &G,
    project_id: ProjectId,
    job_id: JobId,
    pending_ref: Option<&PendingInvestigationRef>,
    precreated_workspace: Option<&Workspace>,
) -> Result<(), UseCaseError>
where
    S: DispatchStore,
    G: DispatchInfraPort,
{
    let Some(pending_ref) = pending_ref else {
        return Ok(());
    };
    if let Err(error) = plan_and_apply_investigation_ref(
        store,
        git_port,
        project_id,
        GitOperationEntityRef::Job(job_id),
        &pending_ref.ref_name,
        &pending_ref.commit_oid,
    )
    .await
    {
        cleanup_failed_dispatch(
            store,
            git_port,
            project_id,
            precreated_workspace,
            Some(&pending_ref.ref_name),
        )
        .await;
        let _ = JobRepository::delete(store, job_id).await;
        return Err(error);
    }
    Ok(())
}

pub async fn maybe_cleanup_investigation_ref<S, G>(
    store: &S,
    git_port: &G,
    project_id: ProjectId,
    finding: &Finding,
) -> Result<(), UseCaseError>
where
    S: FindingRepository + GitOperationRepository + ActivityRepository,
    G: DispatchInfraPort,
{
    if finding.source_step_id != StepId::InvestigateItem
        || finding.source_subject_kind != ingot_domain::finding::FindingSubjectKind::Candidate
    {
        return Ok(());
    }

    let remaining_unresolved = store
        .list_by_item(finding.source_item_id)
        .await
        .map_err(UseCaseError::Repository)?
        .into_iter()
        .any(|candidate| {
            candidate.source_job_id == finding.source_job_id && candidate.triage.is_unresolved()
        });
    if remaining_unresolved {
        return Ok(());
    }

    let ref_name = investigation_ref_name(finding.source_job_id);
    let existing_oid = git_port.resolve_ref_oid(project_id, &ref_name).await?;
    let Some(existing_oid) = existing_oid else {
        return Ok(());
    };

    let mut operation = GitOperation {
        id: GitOperationId::new(),
        project_id,
        entity: GitOperationEntityRef::Job(finding.source_job_id),
        payload: OperationPayload::RemoveInvestigationRef {
            ref_name: ref_name.clone(),
            expected_old_oid: existing_oid,
        },
        status: GitOperationStatus::Planned,
        created_at: Utc::now(),
        completed_at: None,
    };
    create_planned(store, store, &operation, project_id).await?;
    git_port.delete_ref(project_id, &ref_name).await?;
    mark_applied(store, &mut operation).await?;
    Ok(())
}

#[must_use]
pub fn autopilot_dispatch_requires_live_target_head(
    item: &Item,
    revision: &ItemRevision,
    jobs: &[Job],
    findings: &[Finding],
    convergences: &[Convergence],
) -> bool {
    Evaluator::new()
        .evaluate(item, revision, jobs, findings, convergences)
        .dispatchable_step_id
        == Some(StepId::AuthorInitial)
        && revision.seed.seed_commit_oid().is_none()
}

#[must_use]
pub fn should_fill_candidate_subject_from_workspace(step_id: StepId) -> bool {
    matches!(
        step_id,
        StepId::ReviewIncrementalInitial
            | StepId::ReviewCandidateInitial
            | StepId::ReviewCandidateRepair
            | StepId::ValidateCandidateInitial
            | StepId::ValidateCandidateRepair
            | StepId::ReviewAfterIntegrationRepair
            | StepId::ValidateAfterIntegrationRepair
            | StepId::InvestigateItem
    )
}

#[must_use]
pub fn current_authoring_head_for_revision(
    jobs: &[Job],
    revision: &ItemRevision,
) -> Option<CommitOid> {
    crate::authoring_history::current_authoring_head_for_revision(jobs, revision)
}

#[must_use]
pub fn should_rebind_implicit_author_initial_job(
    job: &Job,
    revision: &ItemRevision,
    has_authoring_workspace: bool,
) -> bool {
    job.step_id == StepId::AuthorInitial
        && job.workspace_kind == WorkspaceKind::Authoring
        && job.execution_permission == ExecutionPermission::MayMutate
        && !revision.seed.is_explicit()
        && !has_authoring_workspace
}

#[must_use]
pub fn current_authoring_head_for_revision_with_workspace(
    revision: &ItemRevision,
    jobs: &[Job],
    workspace: Option<&Workspace>,
) -> Option<CommitOid> {
    crate::authoring_history::current_authoring_head_for_revision_with_workspace(
        revision, jobs, workspace,
    )
}

#[must_use]
pub fn effective_authoring_base_commit_oid(
    revision: &ItemRevision,
    workspace: Option<&Workspace>,
) -> Option<CommitOid> {
    crate::authoring_history::effective_authoring_base_commit_oid(revision, workspace)
}

fn needs_mutable_authoring_head(job: &Job) -> bool {
    job.workspace_kind == WorkspaceKind::Authoring
        && job.execution_permission == ExecutionPermission::MayMutate
        && job.job_input.head_commit_oid().is_none()
}

async fn ensure_authoring_workspace_persisted<W, G>(
    workspace_repo: &W,
    git_port: &G,
    project_id: ProjectId,
    revision: &ItemRevision,
    job: &Job,
) -> Result<EnsuredAuthoringWorkspace, UseCaseError>
where
    W: WorkspaceRepository,
    G: DispatchInfraPort,
{
    let existing = workspace_repo
        .find_authoring_for_revision(revision.id)
        .await?;
    let workspace_exists = existing.is_some();
    let workspace = git_port
        .ensure_authoring_workspace(project_id, revision, job, existing)
        .await?;

    if workspace_exists {
        workspace_repo.update(&workspace).await?;
    } else {
        workspace_repo.create(&workspace).await?;
    }

    Ok(EnsuredAuthoringWorkspace {
        workspace,
        created: !workspace_exists,
    })
}

async fn bind_dispatch_subjects_if_needed<W, G>(
    workspace_repo: &W,
    git_port: &G,
    project: &Project,
    revision: &ItemRevision,
    jobs: &[Job],
    job: &mut Job,
    precreated_authoring_workspace: &mut Option<Workspace>,
) -> Result<Option<PendingInvestigationRef>, UseCaseError>
where
    W: WorkspaceRepository,
    G: DispatchInfraPort,
{
    let fills_candidate_subject = should_fill_candidate_subject_from_workspace(job.step_id);

    if needs_mutable_authoring_head(job) {
        let resolved_head = git_port
            .resolve_ref_oid(project.id, &revision.target_ref)
            .await?
            .ok_or_else(|| UseCaseError::TargetRefUnresolved(revision.target_ref.to_string()))?;
        job.job_input = JobInput::authoring_head(resolved_head);
        let ensured_workspace = ensure_authoring_workspace_persisted(
            workspace_repo,
            git_port,
            project.id,
            revision,
            job,
        )
        .await?;
        if ensured_workspace.created {
            *precreated_authoring_workspace = Some(ensured_workspace.workspace);
        }
        return Ok(None);
    }

    let mut base_commit_oid = job.job_input.base_commit_oid().cloned();
    let mut head_commit_oid = job.job_input.head_commit_oid().cloned();

    if fills_candidate_subject {
        let authoring_workspace = workspace_repo
            .find_authoring_for_revision(revision.id)
            .await?;
        if base_commit_oid.is_none() {
            base_commit_oid =
                effective_authoring_base_commit_oid(revision, authoring_workspace.as_ref());
        }
        if head_commit_oid.is_none() {
            head_commit_oid = current_authoring_head_for_revision_with_workspace(
                revision,
                jobs,
                authoring_workspace.as_ref(),
            );
        }
        if let (Some(base_commit_oid), Some(head_commit_oid)) =
            (base_commit_oid.as_ref(), head_commit_oid.as_ref())
        {
            job.job_input =
                JobInput::candidate_subject(base_commit_oid.clone(), head_commit_oid.clone());
            return Ok(None);
        }
    }

    if job.step_id == StepId::InvestigateItem
        && (base_commit_oid.is_none() || head_commit_oid.is_none())
    {
        if let Some(seed_commit_oid) = revision.seed.seed_commit_oid() {
            job.job_input =
                JobInput::candidate_subject(seed_commit_oid.clone(), seed_commit_oid.clone());
            return Ok(None);
        }

        let resolved_head = git_port
            .resolve_ref_oid(project.id, &revision.target_ref)
            .await?
            .ok_or_else(|| UseCaseError::TargetRefUnresolved(revision.target_ref.to_string()))?;
        let ref_name = investigation_ref_name(job.id);
        job.job_input = JobInput::candidate_subject(resolved_head.clone(), resolved_head.clone());
        return Ok(Some(PendingInvestigationRef {
            ref_name,
            commit_oid: resolved_head,
        }));
    }

    if fills_candidate_subject && !(base_commit_oid.is_some() && head_commit_oid.is_some()) {
        return Err(UseCaseError::IllegalStepDispatch(format!(
            "Incomplete candidate subject for step: {}",
            job.step_id
        )));
    }

    Ok(None)
}

fn ensure_dispatch_context_matches(
    project: &Project,
    item: &Item,
    revision: &ItemRevision,
    job: &Job,
) -> Result<(), UseCaseError> {
    if item.project_id != project.id {
        return Err(UseCaseError::IllegalStepDispatch(
            "dispatch item does not belong to project".into(),
        ));
    }
    if revision.id != item.current_revision_id || revision.item_id != item.id {
        return Err(UseCaseError::IllegalStepDispatch(
            "dispatch revision does not match item".into(),
        ));
    }
    if job.project_id != project.id || job.item_id != item.id || job.item_revision_id != revision.id
    {
        return Err(UseCaseError::IllegalStepDispatch(
            "dispatch job does not match project item revision".into(),
        ));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn prepare_and_persist_dispatched_job<S, G>(
    store: &S,
    git_port: &G,
    project: &Project,
    item: &Item,
    revision: &ItemRevision,
    jobs: &[Job],
    mut job: Job,
    activity: DispatchActivityContext,
) -> Result<PreparedDispatchedJob, UseCaseError>
where
    S: DispatchStore,
    G: DispatchInfraPort,
{
    ensure_dispatch_context_matches(project, item, revision, &job)?;

    let mut precreated_authoring_workspace = None;
    let pending_investigation_ref = bind_dispatch_subjects_if_needed(
        store,
        git_port,
        project,
        revision,
        jobs,
        &mut job,
        &mut precreated_authoring_workspace,
    )
    .await?;

    if let Err(error) = JobRepository::create(store, &job).await {
        cleanup_failed_dispatch(
            store,
            git_port,
            project.id,
            precreated_authoring_workspace.as_ref(),
            pending_investigation_ref
                .as_ref()
                .map(|pending| &pending.ref_name),
        )
        .await;
        return Err(UseCaseError::Repository(error));
    }

    apply_pending_investigation_ref_or_cleanup(
        store,
        git_port,
        project.id,
        job.id,
        pending_investigation_ref.as_ref(),
        precreated_authoring_workspace.as_ref(),
    )
    .await?;

    if precreated_authoring_workspace.is_none() && job.workspace_kind == WorkspaceKind::Authoring {
        let _ = ensure_authoring_workspace_persisted(store, git_port, project.id, revision, &job)
            .await?;
    }

    append_job_dispatched_activity_with_context(store, project.id, item.id, &job, &activity)
        .await?;

    Ok(PreparedDispatchedJob { job })
}

fn bind_autopilot_authoring_head_if_needed(
    revision: &ItemRevision,
    jobs: &[Job],
    workspace: Option<&Workspace>,
    author_initial_head_commit_oid: Option<&CommitOid>,
    job: &mut Job,
) -> Result<(), UseCaseError> {
    if !needs_mutable_authoring_head(job) {
        return Ok(());
    }

    let head_commit_oid = match job.step_id {
        StepId::AuthorInitial => author_initial_head_commit_oid.cloned().or_else(|| {
            current_authoring_head_for_revision_with_workspace(revision, jobs, workspace)
        }),
        _ => current_authoring_head_for_revision_with_workspace(revision, jobs, workspace),
    };

    let Some(head_commit_oid) = head_commit_oid else {
        return Err(UseCaseError::Internal(format!(
            "missing authoring head for autopilot-dispatched step {}",
            job.step_id
        )));
    };

    job.job_input = JobInput::authoring_head(head_commit_oid);
    Ok(())
}

async fn append_job_dispatched_activity<A>(
    activity_repo: &A,
    project_id: ProjectId,
    item_id: ItemId,
    job: &Job,
    dispatch_origin: &'static str,
) -> Result<(), UseCaseError>
where
    A: ActivityRepository,
{
    append_job_dispatched_activity_with_context(
        activity_repo,
        project_id,
        item_id,
        job,
        &DispatchActivityContext {
            dispatch_origin: Some(dispatch_origin),
            supersedes_job_id: None,
            retry_no: None,
        },
    )
    .await
}

async fn append_job_dispatched_activity_with_context<A>(
    activity_repo: &A,
    project_id: ProjectId,
    item_id: ItemId,
    job: &Job,
    context: &DispatchActivityContext,
) -> Result<(), UseCaseError>
where
    A: ActivityRepository,
{
    let mut payload = serde_json::Map::new();
    payload.insert("item_id".into(), serde_json::json!(item_id));
    payload.insert("step_id".into(), serde_json::json!(job.step_id));
    if let Some(dispatch_origin) = context.dispatch_origin {
        payload.insert("dispatch_origin".into(), serde_json::json!(dispatch_origin));
    }
    if let Some(supersedes_job_id) = context.supersedes_job_id {
        payload.insert(
            "supersedes_job_id".into(),
            serde_json::json!(supersedes_job_id),
        );
    }
    if let Some(retry_no) = context.retry_no {
        payload.insert("retry_no".into(), serde_json::json!(retry_no));
    }

    activity_repo
        .append(&Activity {
            id: ActivityId::new(),
            project_id,
            event_type: ActivityEventType::JobDispatched,
            subject: ActivitySubject::Job(job.id),
            payload: serde_json::Value::Object(payload),
            created_at: Utc::now(),
        })
        .await
        .map_err(UseCaseError::Repository)
}

/// Returns true if the job's step is closure-relevant (i.e., failures on it should escalate).
pub fn is_closure_relevant_job(job: &Job) -> bool {
    step::find_step(job.step_id).closure_relevance == ClosureRelevance::ClosureRelevant
}

/// Select the most-recent terminal job that produced findings on a
/// closure-relevant step for the given revision.
pub fn latest_closure_findings_job(jobs: &[Job], revision_id: ItemRevisionId) -> Option<&Job> {
    jobs.iter()
        .filter(|job| job.item_revision_id == revision_id)
        .filter(|job| job.state.status().is_terminal())
        .filter(|job| job.state.outcome_class() == Some(OutcomeClass::Findings))
        .filter(|job| is_closure_relevant_job(job))
        .max_by_key(|job| (job.state.ended_at(), job.created_at))
}

/// Returns the escalation reason for a job failure, if applicable.
pub fn failure_escalation_reason(
    job: &Job,
    outcome_class: OutcomeClass,
) -> Option<EscalationReason> {
    if !is_closure_relevant_job(job) {
        return None;
    }

    match outcome_class {
        OutcomeClass::TerminalFailure => Some(EscalationReason::StepFailed),
        OutcomeClass::ProtocolViolation => Some(EscalationReason::ProtocolViolation),
        OutcomeClass::Clean
        | OutcomeClass::Findings
        | OutcomeClass::TransientFailure
        | OutcomeClass::Cancelled => None,
    }
}

/// Maps an outcome class to the terminal job status for failure endpoints.
/// Returns None for outcome classes that are not valid failures (Clean, Findings).
pub fn failure_status(outcome_class: OutcomeClass) -> Option<JobStatus> {
    match outcome_class {
        OutcomeClass::TransientFailure
        | OutcomeClass::TerminalFailure
        | OutcomeClass::ProtocolViolation => Some(JobStatus::Failed),
        OutcomeClass::Cancelled => Some(JobStatus::Cancelled),
        OutcomeClass::Clean | OutcomeClass::Findings => None,
    }
}

/// Returns true if we should clear an item's escalation after a successful retry.
pub fn should_clear_item_escalation_on_success(item: &Item, job: &Job) -> bool {
    item.escalation.is_escalated() && job.retry_no > 0 && is_closure_relevant_job(job)
}

#[allow(clippy::too_many_arguments)]
async fn auto_dispatch_closure_relevant_step<J, W, A>(
    job_repo: &J,
    workspace_repo: &W,
    activity_repo: &A,
    project: &Project,
    item: &Item,
    revision: &ItemRevision,
    jobs: &[Job],
    findings: &[Finding],
    convergences: &[Convergence],
    step_predicate: fn(StepId) -> bool,
    candidate_subject_context: &'static str,
) -> Result<Option<Job>, UseCaseError>
where
    J: JobRepository,
    W: WorkspaceRepository,
    A: ActivityRepository,
{
    let evaluation = Evaluator::new().evaluate(item, revision, jobs, findings, convergences);
    let Some(step_id) = evaluation.dispatchable_step_id else {
        return Ok(None);
    };

    if !step_predicate(step_id) {
        return Ok(None);
    }

    let mut job = dispatch_job(
        item,
        revision,
        jobs,
        findings,
        convergences,
        DispatchJobCommand {
            step_id: Some(step_id),
        },
    )?;

    if should_fill_candidate_subject_from_workspace(job.step_id) {
        let authoring_workspace = workspace_repo
            .find_authoring_for_revision(revision.id)
            .await?;
        job.job_input = build_candidate_subject_input(
            job.step_id,
            &job.job_input,
            revision,
            jobs,
            authoring_workspace.as_ref(),
            candidate_subject_context,
        )?;
    }

    job_repo.create(&job).await?;
    append_job_dispatched_activity(activity_repo, project.id, item.id, &job, "system").await?;

    Ok(Some(job))
}

/// Auto-dispatch a closure-relevant review job if the evaluator recommends one.
///
/// Requires pre-hydrated convergences (with `target_head_valid` set) and pre-loaded entity state.
/// Fills candidate subject from workspace/job history. Creates and persists the job.
///
/// Returns `Some(job)` if a review was dispatched, `None` if not dispatchable.
/// Does NOT handle workspace provisioning or investigation refs — callers do that.
#[allow(clippy::too_many_arguments)]
pub async fn auto_dispatch_review<S>(
    store: &S,
    project: &Project,
    item: &Item,
    revision: &ItemRevision,
    jobs: &[Job],
    findings: &[Finding],
    convergences: &[Convergence],
) -> Result<Option<Job>, UseCaseError>
where
    S: AutoDispatchStore,
{
    auto_dispatch_closure_relevant_step(
        store,
        store,
        store,
        project,
        item,
        revision,
        jobs,
        findings,
        convergences,
        step::is_closure_relevant_review_step,
        "auto-dispatched review",
    )
    .await
}

/// Auto-dispatch a closure-relevant validation job if the evaluator recommends one.
///
/// Requires pre-hydrated convergences (with `target_head_valid` set) and pre-loaded entity state.
/// Fills candidate subject from workspace/job history. Creates and persists the job.
///
/// Returns `Some(job)` if a validation step was dispatched, `None` if not dispatchable.
#[allow(clippy::too_many_arguments)]
pub async fn auto_dispatch_validation<S>(
    store: &S,
    project: &Project,
    item: &Item,
    revision: &ItemRevision,
    jobs: &[Job],
    findings: &[Finding],
    convergences: &[Convergence],
) -> Result<Option<Job>, UseCaseError>
where
    S: AutoDispatchStore,
{
    auto_dispatch_closure_relevant_step(
        store,
        store,
        store,
        project,
        item,
        revision,
        jobs,
        findings,
        convergences,
        step::is_closure_relevant_validate_step,
        "auto-dispatched validation",
    )
    .await
}

/// Auto-dispatch any evaluator-recommended step without the closure-relevance filter.
/// Used when `project.execution_mode == Autopilot`.
///
/// Returns `Some(job)` if dispatched, `None` if no dispatchable step.
/// Human gates (approval, escalation, findings triage) are respected: the evaluator
/// will not set `dispatchable_step_id` when those gates are active.
#[allow(clippy::too_many_arguments)]
pub async fn auto_dispatch_autopilot<S>(
    store: &S,
    project: &Project,
    item: &Item,
    revision: &ItemRevision,
    jobs: &[Job],
    findings: &[Finding],
    convergences: &[Convergence],
    author_initial_head_commit_oid: Option<CommitOid>,
) -> Result<Option<Job>, UseCaseError>
where
    S: AutoDispatchStore,
{
    let evaluation = Evaluator::new().evaluate(item, revision, jobs, findings, convergences);
    let Some(step_id) = evaluation.dispatchable_step_id else {
        return Ok(None);
    };

    let mut job = dispatch_job(
        item,
        revision,
        jobs,
        findings,
        convergences,
        DispatchJobCommand {
            step_id: Some(step_id),
        },
    )?;

    let needs_authoring_workspace = should_fill_candidate_subject_from_workspace(job.step_id)
        || needs_mutable_authoring_head(&job);
    let authoring_workspace = if needs_authoring_workspace {
        store.find_authoring_for_revision(revision.id).await?
    } else {
        None
    };

    bind_autopilot_authoring_head_if_needed(
        revision,
        jobs,
        authoring_workspace.as_ref(),
        author_initial_head_commit_oid.as_ref(),
        &mut job,
    )?;

    if should_fill_candidate_subject_from_workspace(job.step_id) {
        job.job_input = build_candidate_subject_input(
            job.step_id,
            &job.job_input,
            revision,
            jobs,
            authoring_workspace.as_ref(),
            "autopilot-dispatched step",
        )?;
    }

    JobRepository::create(store, &job).await?;
    append_job_dispatched_activity(store, project.id, item.id, &job, "autopilot").await?;

    Ok(Some(job))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use ingot_domain::git_ref::GitRef;
    use ingot_domain::ids::{ItemId, ItemRevisionId, ProjectId, WorkspaceId};
    use ingot_domain::item::ApprovalState;
    use ingot_domain::job::{ExecutionPermission, Job, JobInput, OutputArtifactKind, PhaseKind};
    use ingot_domain::ports::GitOperationRepository;
    use ingot_domain::project::ExecutionMode;
    use ingot_domain::revision::{ApprovalPolicy, AuthoringBaseSeed};
    use ingot_domain::test_support::{
        ItemBuilder, JobBuilder, ProjectBuilder, RevisionBuilder, WorkspaceBuilder,
    };
    use ingot_domain::workspace::WorkspaceStatus;
    use ingot_test_support::sqlite::migrated_test_db;
    use uuid::Uuid;

    use super::*;

    #[derive(Clone, Debug)]
    struct FakeDispatchInfra {
        resolved_oid: Option<CommitOid>,
    }

    impl FakeDispatchInfra {
        fn resolving(commit_oid: impl Into<String>) -> Self {
            Self {
                resolved_oid: Some(CommitOid::new(commit_oid.into())),
            }
        }
    }

    impl DispatchInfraPort for FakeDispatchInfra {
        async fn resolve_ref_oid(
            &self,
            _project_id: ProjectId,
            _ref_name: &GitRef,
        ) -> Result<Option<CommitOid>, UseCaseError> {
            Ok(self.resolved_oid.clone())
        }

        async fn update_ref(
            &self,
            _project_id: ProjectId,
            _ref_name: &GitRef,
            _commit_oid: &CommitOid,
        ) -> Result<(), UseCaseError> {
            Ok(())
        }

        async fn delete_ref(
            &self,
            _project_id: ProjectId,
            _ref_name: &GitRef,
        ) -> Result<(), UseCaseError> {
            Ok(())
        }

        async fn remove_workspace_files(
            &self,
            _project_id: ProjectId,
            _workspace: &Workspace,
        ) -> Result<(), UseCaseError> {
            Ok(())
        }

        async fn ensure_authoring_workspace(
            &self,
            project_id: ProjectId,
            revision: &ItemRevision,
            job: &Job,
            existing: Option<Workspace>,
        ) -> Result<Workspace, UseCaseError> {
            if let Some(workspace) = existing {
                return Ok(workspace);
            }

            let head_commit_oid = job
                .job_input
                .head_commit_oid()
                .cloned()
                .ok_or_else(|| UseCaseError::Internal("test job missing head".into()))?;
            Ok(WorkspaceBuilder::new(project_id, WorkspaceKind::Authoring)
                .created_for_revision_id(revision.id)
                .base_commit_oid(head_commit_oid.to_string())
                .head_commit_oid(head_commit_oid.to_string())
                .build())
        }
    }

    fn test_project() -> Project {
        ProjectBuilder::new(
            std::env::temp_dir().join(format!("ingot-usecases-dispatch-{}", Uuid::now_v7())),
        )
        .build()
    }

    fn test_job(step_id: StepId, output_artifact_kind: OutputArtifactKind) -> Job {
        JobBuilder::new(
            ProjectId::from_uuid(Uuid::nil()),
            ItemId::from_uuid(Uuid::nil()),
            ItemRevisionId::from_uuid(Uuid::nil()),
            step_id,
        )
        .phase_kind(PhaseKind::Author)
        .workspace_kind(WorkspaceKind::Authoring)
        .execution_permission(ExecutionPermission::MayMutate)
        .phase_template_slug("author-initial")
        .job_input(JobInput::authoring_head(CommitOid::from("head")))
        .output_artifact_kind(output_artifact_kind)
        .build()
    }

    #[test]
    fn authoring_head_from_latest_completed_commit_job() {
        let item_id = ItemId::from_uuid(Uuid::nil());
        let revision_id = ItemRevisionId::from_uuid(Uuid::nil());
        let project_id = ProjectId::from_uuid(Uuid::nil());
        let now = Utc::now();
        let revision = RevisionBuilder::new(item_id)
            .id(revision_id)
            .explicit_seed("seed")
            .created_at(now)
            .build();
        let job = JobBuilder::new(project_id, item_id, revision_id, "author_initial")
            .status(ingot_domain::job::JobStatus::Completed)
            .outcome_class(ingot_domain::job::OutcomeClass::Clean)
            .output_artifact_kind(OutputArtifactKind::Commit)
            .output_commit_oid("abc123")
            .created_at(now)
            .started_at(now)
            .ended_at(now)
            .build();

        assert_eq!(
            current_authoring_head_for_revision(&[job], &revision),
            Some("abc123".into())
        );
    }

    #[test]
    fn authoring_head_falls_back_to_seed_commit() {
        let item_id = ItemId::from_uuid(Uuid::nil());
        let revision_id = ItemRevisionId::from_uuid(Uuid::nil());
        let now = Utc::now();
        let revision = RevisionBuilder::new(item_id)
            .id(revision_id)
            .explicit_seed("seed")
            .created_at(now)
            .build();

        assert_eq!(
            current_authoring_head_for_revision(&[], &revision),
            Some("seed".into())
        );
    }

    #[test]
    fn should_fill_is_true_for_review_steps() {
        assert!(should_fill_candidate_subject_from_workspace(
            StepId::ReviewIncrementalInitial
        ));
        assert!(should_fill_candidate_subject_from_workspace(
            StepId::InvestigateItem
        ));
    }

    #[test]
    fn should_fill_is_false_for_authoring_steps() {
        assert!(!should_fill_candidate_subject_from_workspace(
            StepId::AuthorInitial
        ));
    }

    #[tokio::test]
    async fn bind_dispatch_subjects_does_not_persist_investigation_ref_before_job_creation() {
        let db = migrated_test_db("ingot-usecases-dispatch").await;
        let project = test_project();
        db.create_project(&project).await.expect("persist project");

        let item = ItemBuilder::new(project.id, ItemRevisionId::from_uuid(Uuid::now_v7())).build();
        let revision = RevisionBuilder::new(item.id)
            .id(item.current_revision_id)
            .seed_target_commit_oid(Some("target-head".to_string()))
            .build();
        db.create_item_with_revision(&item, &revision)
            .await
            .expect("persist item");

        let mut job = test_job(StepId::InvestigateItem, OutputArtifactKind::FindingReport);
        job.project_id = project.id;
        job.item_id = item.id;
        job.item_revision_id = revision.id;
        job.workspace_kind = WorkspaceKind::Review;
        job.execution_permission = ExecutionPermission::MustNotMutate;
        job.phase_kind = PhaseKind::Investigate;
        job.job_input = JobInput::None;

        let infra = FakeDispatchInfra::resolving("target-head");
        let mut precreated_authoring_workspace = None;
        let pending_investigation_ref = bind_dispatch_subjects_if_needed(
            &db,
            &infra,
            &project,
            &revision,
            &[],
            &mut job,
            &mut precreated_authoring_workspace,
        )
        .await
        .expect("bind dispatch subjects")
        .expect("expected pending investigation ref");

        assert!(precreated_authoring_workspace.is_none());
        let expected_oid = CommitOid::new("target-head");
        assert_eq!(job.job_input.base_commit_oid(), Some(&expected_oid));
        assert_eq!(job.job_input.head_commit_oid(), Some(&expected_oid));

        let operations = GitOperationRepository::find_unresolved(&db)
            .await
            .expect("list git operations");
        assert!(
            operations.iter().all(|operation| !matches!(
                &operation.payload,
                OperationPayload::CreateInvestigationRef { ref_name, .. }
                    if *ref_name == pending_investigation_ref.ref_name
            )),
            "pending investigation ref should not be persisted during binding"
        );
    }

    #[tokio::test]
    async fn bind_dispatch_subjects_falls_back_when_workspace_subject_is_partial() {
        let db = migrated_test_db("ingot-usecases-dispatch").await;
        let project = test_project();
        db.create_project(&project).await.expect("persist project");

        let item = ItemBuilder::new(project.id, ItemRevisionId::from_uuid(Uuid::now_v7())).build();
        let revision = RevisionBuilder::new(item.id)
            .id(item.current_revision_id)
            .seed_target_commit_oid(Some("target-head".to_string()))
            .build();
        db.create_item_with_revision(&item, &revision)
            .await
            .expect("persist item");
        let partial_workspace = WorkspaceBuilder::new(project.id, WorkspaceKind::Authoring)
            .id(WorkspaceId::from_uuid(Uuid::now_v7()))
            .created_for_revision_id(revision.id)
            .status(WorkspaceStatus::Provisioning)
            .created_at(Utc::now())
            .build();
        db.create_workspace(&partial_workspace)
            .await
            .expect("persist partial workspace");

        let mut job = test_job(StepId::InvestigateItem, OutputArtifactKind::FindingReport);
        job.project_id = project.id;
        job.item_id = item.id;
        job.item_revision_id = revision.id;
        job.workspace_kind = WorkspaceKind::Review;
        job.execution_permission = ExecutionPermission::MustNotMutate;
        job.phase_kind = PhaseKind::Investigate;
        job.job_input = JobInput::None;

        let infra = FakeDispatchInfra::resolving("target-head");
        let mut precreated_authoring_workspace = None;
        let pending_investigation_ref = bind_dispatch_subjects_if_needed(
            &db,
            &infra,
            &project,
            &revision,
            &[],
            &mut job,
            &mut precreated_authoring_workspace,
        )
        .await
        .expect("bind dispatch subjects")
        .expect("expected pending investigation ref");

        assert!(precreated_authoring_workspace.is_none());
        assert_eq!(
            pending_investigation_ref.ref_name,
            investigation_ref_name(job.id)
        );
        let expected_oid = CommitOid::new("target-head");
        assert_eq!(job.job_input.base_commit_oid(), Some(&expected_oid));
        assert_eq!(job.job_input.head_commit_oid(), Some(&expected_oid));
    }

    #[tokio::test]
    async fn bind_dispatch_subjects_rejects_partial_review_subject() {
        let db = migrated_test_db("ingot-usecases-dispatch").await;
        let project = test_project();
        db.create_project(&project).await.expect("persist project");

        let item = ItemBuilder::new(project.id, ItemRevisionId::from_uuid(Uuid::now_v7())).build();
        let revision = RevisionBuilder::new(item.id)
            .id(item.current_revision_id)
            .seed_target_commit_oid(Some("target-head".to_string()))
            .build();
        db.create_item_with_revision(&item, &revision)
            .await
            .expect("persist item");
        let partial_workspace = WorkspaceBuilder::new(project.id, WorkspaceKind::Authoring)
            .id(WorkspaceId::from_uuid(Uuid::now_v7()))
            .created_for_revision_id(revision.id)
            .status(WorkspaceStatus::Provisioning)
            .created_at(Utc::now())
            .build();
        db.create_workspace(&partial_workspace)
            .await
            .expect("persist partial workspace");

        let mut job = test_job(
            StepId::ReviewIncrementalInitial,
            OutputArtifactKind::ReviewReport,
        );
        job.project_id = project.id;
        job.item_id = item.id;
        job.item_revision_id = revision.id;
        job.workspace_kind = WorkspaceKind::Review;
        job.execution_permission = ExecutionPermission::MustNotMutate;
        job.phase_kind = PhaseKind::Review;
        job.job_input = JobInput::None;

        let infra = FakeDispatchInfra::resolving("target-head");
        let mut precreated_authoring_workspace = None;
        let result = bind_dispatch_subjects_if_needed(
            &db,
            &infra,
            &project,
            &revision,
            &[],
            &mut job,
            &mut precreated_authoring_workspace,
        )
        .await;

        assert!(matches!(
            result,
            Err(UseCaseError::IllegalStepDispatch(message))
                if message.contains("Incomplete candidate subject")
        ));
    }

    #[tokio::test]
    async fn prepare_dispatched_job_rejects_mismatched_context_before_side_effects() {
        let db = migrated_test_db("ingot-usecases-dispatch").await;
        let project = test_project();
        db.create_project(&project).await.expect("persist project");

        let item = ItemBuilder::new(project.id, ItemRevisionId::from_uuid(Uuid::now_v7())).build();
        let revision = RevisionBuilder::new(item.id)
            .id(item.current_revision_id)
            .build();
        db.create_item_with_revision(&item, &revision)
            .await
            .expect("persist item");

        let mut job = test_job(StepId::AuthorInitial, OutputArtifactKind::Commit);
        job.project_id = project.id;
        job.item_revision_id = revision.id;
        job.item_id = ItemId::from_uuid(Uuid::now_v7());

        let error = prepare_and_persist_dispatched_job(
            &db,
            &FakeDispatchInfra::resolving("target-head"),
            &project,
            &item,
            &revision,
            &[],
            job,
            DispatchActivityContext {
                dispatch_origin: Some("operator"),
                supersedes_job_id: None,
                retry_no: None,
            },
        )
        .await
        .expect_err("mismatched job context should be rejected");

        assert!(matches!(
            error,
            UseCaseError::IllegalStepDispatch(message)
                if message.contains("dispatch job does not match project item revision")
        ));
        assert!(
            db.list_jobs_by_item(item.id)
                .await
                .expect("list jobs")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn prepare_dispatched_job_does_not_delete_existing_workspace_when_job_create_fails() {
        let db = migrated_test_db("ingot-usecases-dispatch").await;
        let project = test_project();
        db.create_project(&project).await.expect("persist project");

        let item = ItemBuilder::new(project.id, ItemRevisionId::from_uuid(Uuid::now_v7())).build();
        let revision = RevisionBuilder::new(item.id)
            .id(item.current_revision_id)
            .seed_target_commit_oid(Some("target-head".to_string()))
            .build();
        db.create_item_with_revision(&item, &revision)
            .await
            .expect("persist item");

        let existing_workspace = WorkspaceBuilder::new(project.id, WorkspaceKind::Authoring)
            .id(WorkspaceId::from_uuid(Uuid::now_v7()))
            .created_for_revision_id(revision.id)
            .base_commit_oid("workspace-base")
            .head_commit_oid("workspace-head")
            .status(WorkspaceStatus::Ready)
            .created_at(Utc::now())
            .build();
        db.create_workspace(&existing_workspace)
            .await
            .expect("persist existing workspace");

        let mut job = test_job(StepId::AuthorInitial, OutputArtifactKind::Commit);
        job.project_id = project.id;
        job.item_id = item.id;
        job.item_revision_id = revision.id;
        job.job_input = JobInput::None;
        db.create_job(&job)
            .await
            .expect("persist duplicate job blocker");

        let error = prepare_and_persist_dispatched_job(
            &db,
            &FakeDispatchInfra::resolving("target-head"),
            &project,
            &item,
            &revision,
            &[],
            job,
            DispatchActivityContext {
                dispatch_origin: Some("operator"),
                supersedes_job_id: None,
                retry_no: None,
            },
        )
        .await
        .expect_err("duplicate job id should fail persistence");

        assert!(matches!(error, UseCaseError::Repository(_)));
        let workspace = db
            .get_workspace(existing_workspace.id)
            .await
            .expect("existing workspace should remain");
        assert_eq!(workspace.id, existing_workspace.id);
    }

    #[test]
    fn implicit_autopilot_author_initial_requires_live_head() {
        let item_id = ItemId::from_uuid(Uuid::nil());
        let revision_id = ItemRevisionId::from_uuid(Uuid::nil());
        let project_id = ProjectId::from_uuid(Uuid::nil());
        let item = ItemBuilder::new(project_id, revision_id)
            .id(item_id)
            .build();
        let revision = RevisionBuilder::new(item_id)
            .id(revision_id)
            .seed_commit_oid(None::<String>)
            .seed_target_commit_oid(Some("target-head".to_string()))
            .build();

        assert!(autopilot_dispatch_requires_live_target_head(
            &item,
            &revision,
            &[],
            &[],
            &[]
        ));
    }

    #[test]
    fn implicit_author_initial_rebind_only_applies_without_workspace() {
        let item_id = ItemId::from_uuid(Uuid::nil());
        let revision_id = ItemRevisionId::from_uuid(Uuid::nil());
        let project_id = ProjectId::from_uuid(Uuid::nil());
        let revision = RevisionBuilder::new(item_id)
            .id(revision_id)
            .seed_commit_oid(None::<String>)
            .seed_target_commit_oid(Some("target-head".to_string()))
            .build();
        let job = JobBuilder::new(project_id, item_id, revision_id, StepId::AuthorInitial)
            .workspace_kind(WorkspaceKind::Authoring)
            .execution_permission(ExecutionPermission::MayMutate)
            .build();

        assert!(should_rebind_implicit_author_initial_job(
            &job, &revision, false
        ));
        assert!(!should_rebind_implicit_author_initial_job(
            &job, &revision, true
        ));
    }

    #[tokio::test]
    async fn autopilot_dispatch_binds_author_initial_from_implicit_target_head() {
        let db = migrated_test_db("ingot-usecases-dispatch").await;
        let project_id = ProjectId::new();
        let item_id = ItemId::new();
        let revision_id = ItemRevisionId::new();

        let project = ProjectBuilder::new(
            std::env::temp_dir().join(format!("ingot-usecases-dispatch-{}", Uuid::now_v7())),
        )
        .id(project_id)
        .execution_mode(ExecutionMode::Autopilot)
        .build();
        let item = ItemBuilder::new(project_id, revision_id)
            .id(item_id)
            .approval_state(ApprovalState::NotRequired)
            .build();
        let revision = RevisionBuilder::new(item_id)
            .id(revision_id)
            .approval_policy(ApprovalPolicy::NotRequired)
            .seed(AuthoringBaseSeed::Implicit {
                seed_target_commit_oid: "target-head".into(),
            })
            .template_map_snapshot(serde_json::json!({"author_initial":"author-initial"}))
            .build();

        db.create_project(&project).await.expect("persist project");
        db.create_item_with_revision(&item, &revision)
            .await
            .expect("persist item");

        let job = auto_dispatch_autopilot(
            &db,
            &project,
            &item,
            &revision,
            &[],
            &[],
            &[],
            Some("target-head".into()),
        )
        .await
        .expect("autopilot dispatch")
        .expect("author_initial job");

        assert_eq!(job.step_id, StepId::AuthorInitial);
        assert_eq!(
            job.job_input,
            JobInput::authoring_head("target-head".into())
        );
    }

    #[tokio::test]
    async fn autopilot_dispatch_rejects_implicit_author_initial_without_live_head() {
        let db = migrated_test_db("ingot-usecases-dispatch").await;
        let project_id = ProjectId::new();
        let item_id = ItemId::new();
        let revision_id = ItemRevisionId::new();

        let project = ProjectBuilder::new(
            std::env::temp_dir().join(format!("ingot-usecases-dispatch-{}", Uuid::now_v7())),
        )
        .id(project_id)
        .execution_mode(ExecutionMode::Autopilot)
        .build();
        let item = ItemBuilder::new(project_id, revision_id)
            .id(item_id)
            .approval_state(ApprovalState::NotRequired)
            .build();
        let revision = RevisionBuilder::new(item_id)
            .id(revision_id)
            .approval_policy(ApprovalPolicy::NotRequired)
            .seed(AuthoringBaseSeed::Implicit {
                seed_target_commit_oid: "stale-seed-target".into(),
            })
            .template_map_snapshot(serde_json::json!({"author_initial":"author-initial"}))
            .build();

        db.create_project(&project).await.expect("persist project");
        db.create_item_with_revision(&item, &revision)
            .await
            .expect("persist item");

        let error = auto_dispatch_autopilot(&db, &project, &item, &revision, &[], &[], &[], None)
            .await
            .expect_err("implicit author_initial requires a live target head");

        assert!(
            error
                .to_string()
                .contains("missing authoring head for autopilot-dispatched step author_initial")
        );
    }
}
