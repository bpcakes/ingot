mod types {
    use std::future::Future;

    use ingot_domain::commit_oid::CommitOid;
    use ingot_domain::convergence::Convergence;
    use ingot_domain::convergence_queue::ConvergenceQueueEntry;
    use ingot_domain::finding::Finding;
    use ingot_domain::git_operation::GitOperation;
    use ingot_domain::ids::{ItemId, ProjectId};
    use ingot_domain::item::Item;
    use ingot_domain::job::Job;
    use ingot_domain::ports::FinalizationMutation;
    use ingot_domain::project::Project;
    use ingot_domain::revision::ItemRevision;

    use crate::UseCaseError;

    #[derive(Debug, Clone)]
    pub struct ConvergenceQueuePrepareContext {
        pub project: Project,
        pub item: Item,
        pub revision: ItemRevision,
        pub jobs: Vec<Job>,
        pub findings: Vec<Finding>,
        pub convergences: Vec<Convergence>,
        pub active_queue_entry: Option<ConvergenceQueueEntry>,
        pub lane_head: Option<ConvergenceQueueEntry>,
    }

    #[derive(Debug, Clone)]
    pub struct SystemActionItemState {
        pub item_id: ItemId,
        pub item: ingot_domain::item::Item,
        pub revision: ItemRevision,
        pub jobs: Vec<Job>,
        pub findings: Vec<Finding>,
        pub convergences: Vec<Convergence>,
        pub queue_entry: Option<ConvergenceQueueEntry>,
    }

    #[derive(Debug, Clone)]
    pub struct SystemActionProjectState {
        pub project: Project,
        pub items: Vec<SystemActionItemState>,
    }

    #[derive(Debug, Clone)]
    pub struct ConvergenceApprovalContext {
        pub project: Project,
        pub item: ingot_domain::item::Item,
        pub revision: ItemRevision,
        pub has_active_job: bool,
        pub has_active_convergence: bool,
        pub finalize_readiness: ApprovalFinalizeReadiness,
    }

    #[derive(Debug, Clone)]
    pub enum ApprovalFinalizeReadiness {
        MissingPreparedConvergence,
        PreparedConvergenceStale,
        ConvergenceNotQueued,
        ConvergenceNotLaneHead,
        Ready {
            convergence: Box<Convergence>,
            queue_entry: ConvergenceQueueEntry,
        },
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum FinalizePreparedTrigger {
        ApprovalCommand,
        SystemCommand,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum CheckoutFinalizationReadiness {
        Blocked { message: String },
        NeedsSync,
        Synced,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum FinalizeTargetRefResult {
        AlreadyFinalized,
        UpdatedNow,
        Stale,
    }

    #[derive(Debug, Clone, Default)]
    pub struct RejectApprovalTeardown {
        pub has_cancelled_convergence: bool,
        pub has_cancelled_queue_entry: bool,
        pub first_cancelled_convergence_id: Option<String>,
        pub first_cancelled_queue_entry_id: Option<String>,
    }

    #[derive(Debug, Clone)]
    pub struct RejectApprovalContext {
        pub item: ingot_domain::item::Item,
        pub has_active_job: bool,
        pub has_active_convergence: bool,
    }

    pub trait ConvergenceCommandPort: Send + Sync {
        fn load_queue_prepare_context(
            &self,
            project_id: ProjectId,
            item_id: ItemId,
        ) -> impl Future<Output = Result<ConvergenceQueuePrepareContext, UseCaseError>> + Send;

        fn create_queue_entry(
            &self,
            queue_entry: &ConvergenceQueueEntry,
        ) -> impl Future<Output = Result<(), UseCaseError>> + Send;

        fn update_queue_entry(
            &self,
            queue_entry: &ConvergenceQueueEntry,
        ) -> impl Future<Output = Result<(), UseCaseError>> + Send;

        fn append_activity(
            &self,
            activity: &ingot_domain::activity::Activity,
        ) -> impl Future<Output = Result<(), UseCaseError>> + Send;

        fn load_approval_context(
            &self,
            project_id: ProjectId,
            item_id: ItemId,
        ) -> impl Future<Output = Result<ConvergenceApprovalContext, UseCaseError>> + Send;

        fn update_item(
            &self,
            item: &ingot_domain::item::Item,
        ) -> impl Future<Output = Result<(), UseCaseError>> + Send;

        fn load_reject_approval_context(
            &self,
            project_id: ProjectId,
            item_id: ItemId,
        ) -> impl Future<Output = Result<RejectApprovalContext, UseCaseError>> + Send;

        fn teardown_reject_approval(
            &self,
            project_id: ProjectId,
            item_id: ItemId,
        ) -> impl Future<Output = Result<RejectApprovalTeardown, UseCaseError>> + Send;

        fn apply_rejected_approval(
            &self,
            item: &ingot_domain::item::Item,
            next_revision: &ItemRevision,
        ) -> impl Future<Output = Result<(), UseCaseError>> + Send;
    }

    pub trait ConvergenceSystemActionPort: Send + Sync {
        fn load_system_action_projects(
            &self,
        ) -> impl Future<Output = Result<Vec<SystemActionProjectState>, UseCaseError>> + Send;

        fn promote_queue_heads(
            &self,
            project_id: ProjectId,
        ) -> impl Future<Output = Result<(), UseCaseError>> + Send;

        fn prepare_queue_head_convergence(
            &self,
            project: &Project,
            state: &SystemActionItemState,
            queue_entry: &ConvergenceQueueEntry,
        ) -> impl Future<Output = Result<(), UseCaseError>> + Send;

        fn invalidate_prepared_convergence(
            &self,
            project_id: ProjectId,
            item_id: ItemId,
        ) -> impl Future<Output = Result<(), UseCaseError>> + Send;

        fn auto_finalize_prepared_convergence(
            &self,
            project_id: ProjectId,
            item_id: ItemId,
        ) -> impl Future<Output = Result<bool, UseCaseError>> + Send;

        fn auto_queue_convergence(
            &self,
            project_id: ProjectId,
            item_id: ItemId,
        ) -> impl Future<Output = Result<bool, UseCaseError>> + Send;
    }

    pub trait PreparedConvergenceFinalizePort: Send + Sync {
        fn find_or_create_finalize_operation(
            &self,
            operation: &GitOperation,
        ) -> impl Future<Output = Result<GitOperation, UseCaseError>> + Send;

        fn finalize_target_ref(
            &self,
            project: &Project,
            convergence: &Convergence,
        ) -> impl Future<Output = Result<FinalizeTargetRefResult, UseCaseError>> + Send;

        fn checkout_finalization_readiness(
            &self,
            project: &Project,
            item: &ingot_domain::item::Item,
            revision: &ItemRevision,
            prepared_commit_oid: &CommitOid,
        ) -> impl Future<Output = Result<CheckoutFinalizationReadiness, UseCaseError>> + Send;

        fn sync_checkout_to_prepared_commit(
            &self,
            project: &Project,
            revision: &ItemRevision,
            prepared_commit_oid: &CommitOid,
        ) -> impl Future<Output = Result<(), UseCaseError>> + Send;

        fn update_git_operation(
            &self,
            operation: &GitOperation,
        ) -> impl Future<Output = Result<(), UseCaseError>> + Send;

        fn apply_finalization_mutation(
            &self,
            mutation: FinalizationMutation,
        ) -> impl Future<Output = Result<(), UseCaseError>> + Send;
    }
}

pub use types::{
    ApprovalFinalizeReadiness, CheckoutFinalizationReadiness, ConvergenceApprovalContext,
    ConvergenceCommandPort, ConvergenceQueuePrepareContext, ConvergenceSystemActionPort,
    FinalizePreparedTrigger, FinalizeTargetRefResult, PreparedConvergenceFinalizePort,
    RejectApprovalContext, RejectApprovalTeardown, SystemActionItemState, SystemActionProjectState,
};

mod context {
    use ingot_domain::commit_oid::CommitOid;
    use ingot_domain::convergence::{Convergence, ConvergenceStatus};
    use ingot_domain::convergence_queue::{ConvergenceQueueEntry, ConvergenceQueueEntryStatus};
    use ingot_domain::ids::{ItemRevisionId, ProjectId};
    use ingot_domain::item::Item;
    use ingot_domain::job::Job;
    use ingot_domain::project::Project;
    use ingot_domain::revision::ItemRevision;

    use crate::UseCaseError;

    use super::types::{
        ApprovalFinalizeReadiness, ConvergenceApprovalContext, RejectApprovalContext,
    };

    pub fn build_convergence_approval_context(
        project: Project,
        item: Item,
        revision: ItemRevision,
        jobs: &[Job],
        convergences: &[Convergence],
        queue_entry: Option<ConvergenceQueueEntry>,
        resolved_target_oid: Option<&CommitOid>,
    ) -> Result<ConvergenceApprovalContext, UseCaseError> {
        ensure_item_in_project(&item, project.id)?;

        let revision_id = revision.id;
        let prepared_convergence = prepared_convergence_for_revision(convergences, revision_id);

        Ok(ConvergenceApprovalContext {
            project,
            item,
            revision,
            has_active_job: has_active_job_for_revision(jobs, revision_id),
            has_active_convergence: has_active_convergence_for_revision(convergences, revision_id),
            finalize_readiness: approval_finalize_readiness(
                prepared_convergence,
                queue_entry,
                resolved_target_oid,
            ),
        })
    }

    pub fn build_reject_approval_context(
        project_id: ProjectId,
        item: Item,
        revision: &ItemRevision,
        jobs: &[Job],
        convergences: &[Convergence],
    ) -> Result<RejectApprovalContext, UseCaseError> {
        ensure_item_in_project(&item, project_id)?;

        Ok(RejectApprovalContext {
            item,
            has_active_job: has_active_job_for_revision(jobs, revision.id),
            has_active_convergence: has_active_convergence_for_revision(convergences, revision.id),
        })
    }

    fn approval_finalize_readiness(
        prepared_convergence: Option<Convergence>,
        queue_entry: Option<ConvergenceQueueEntry>,
        resolved_target_oid: Option<&CommitOid>,
    ) -> ApprovalFinalizeReadiness {
        let Some(convergence) = prepared_convergence else {
            return ApprovalFinalizeReadiness::MissingPreparedConvergence;
        };

        if !target_matches_convergence_input_or_output(&convergence, resolved_target_oid) {
            return ApprovalFinalizeReadiness::PreparedConvergenceStale;
        }

        let Some(queue_entry) = queue_entry else {
            return ApprovalFinalizeReadiness::ConvergenceNotQueued;
        };
        if queue_entry.status != ConvergenceQueueEntryStatus::Head {
            return ApprovalFinalizeReadiness::ConvergenceNotLaneHead;
        }

        ApprovalFinalizeReadiness::Ready {
            convergence: Box::new(convergence),
            queue_entry,
        }
    }

    fn ensure_item_in_project(item: &Item, project_id: ProjectId) -> Result<(), UseCaseError> {
        if item.project_id == project_id {
            Ok(())
        } else {
            Err(UseCaseError::ItemNotFound)
        }
    }

    fn has_active_job_for_revision(jobs: &[Job], revision_id: ItemRevisionId) -> bool {
        jobs.iter()
            .any(|job| job.item_revision_id == revision_id && job.state.is_active())
    }

    fn has_active_convergence_for_revision(
        convergences: &[Convergence],
        revision_id: ItemRevisionId,
    ) -> bool {
        convergences.iter().any(|convergence| {
            convergence.item_revision_id == revision_id
                && matches!(
                    convergence.state.status(),
                    ConvergenceStatus::Queued | ConvergenceStatus::Running
                )
        })
    }

    fn prepared_convergence_for_revision(
        convergences: &[Convergence],
        revision_id: ItemRevisionId,
    ) -> Option<Convergence> {
        convergences
            .iter()
            .find(|convergence| {
                convergence.item_revision_id == revision_id
                    && convergence.state.status() == ConvergenceStatus::Prepared
            })
            .cloned()
    }

    fn target_matches_convergence_input_or_output(
        convergence: &Convergence,
        resolved_target_oid: Option<&CommitOid>,
    ) -> bool {
        convergence
            .state
            .input_target_commit_oid()
            .zip(convergence.state.prepared_commit_oid())
            .is_some_and(|(input_target_commit_oid, prepared_commit_oid)| {
                resolved_target_oid == Some(input_target_commit_oid)
                    || resolved_target_oid == Some(prepared_commit_oid)
            })
    }

    #[cfg(test)]
    mod tests {
        use ingot_domain::convergence::ConvergenceStatus;
        use ingot_domain::convergence_queue::ConvergenceQueueEntryStatus;
        use ingot_domain::ids::{ItemRevisionId, ProjectId};
        use ingot_domain::item::ApprovalState;
        use ingot_domain::job::JobStatus;
        use ingot_domain::test_support::{
            ConvergenceBuilder, ConvergenceQueueEntryBuilder, ItemBuilder, JobBuilder,
            ProjectBuilder, RevisionBuilder,
        };

        use super::*;

        fn approval_context_parts() -> (Project, Item, ItemRevision) {
            let project_id = ProjectId::new();
            let revision_id = ItemRevisionId::new();
            let item = ItemBuilder::new(project_id, revision_id)
                .approval_state(ApprovalState::Pending)
                .build();
            let project = ProjectBuilder::new("/tmp/ingot-context-test")
                .id(project_id)
                .build();
            let revision = RevisionBuilder::new(item.id).id(revision_id).build();
            (project, item, revision)
        }

        #[test]
        fn approval_context_reports_ready_for_head_queue_and_valid_target() {
            let (project, item, revision) = approval_context_parts();
            let convergence = ConvergenceBuilder::new(project.id, item.id, revision.id)
                .input_target_commit_oid("base")
                .prepared_commit_oid("prepared")
                .build();
            let queue_entry = ConvergenceQueueEntryBuilder::new(project.id, item.id, revision.id)
                .status(ConvergenceQueueEntryStatus::Head)
                .build();

            let context = build_convergence_approval_context(
                project,
                item,
                revision,
                &[],
                &[convergence],
                Some(queue_entry),
                Some(&CommitOid::from("base")),
            )
            .expect("approval context");

            assert!(matches!(
                context.finalize_readiness,
                ApprovalFinalizeReadiness::Ready { .. }
            ));
            assert!(!context.has_active_job);
            assert!(!context.has_active_convergence);
        }

        #[test]
        fn approval_context_marks_prepared_convergence_stale_for_moved_target() {
            let (project, item, revision) = approval_context_parts();
            let convergence = ConvergenceBuilder::new(project.id, item.id, revision.id)
                .input_target_commit_oid("base")
                .prepared_commit_oid("prepared")
                .build();
            let queue_entry = ConvergenceQueueEntryBuilder::new(project.id, item.id, revision.id)
                .status(ConvergenceQueueEntryStatus::Head)
                .build();

            let context = build_convergence_approval_context(
                project,
                item,
                revision,
                &[],
                &[convergence],
                Some(queue_entry),
                Some(&CommitOid::from("other")),
            )
            .expect("approval context");

            assert!(matches!(
                context.finalize_readiness,
                ApprovalFinalizeReadiness::PreparedConvergenceStale
            ));
        }

        #[test]
        fn reject_context_detects_active_work_on_revision() {
            let (project, item, revision) = approval_context_parts();
            let active_job =
                JobBuilder::new(project.id, item.id, revision.id, "validate_integrated")
                    .status(JobStatus::Running)
                    .build();
            let active_convergence = ConvergenceBuilder::new(project.id, item.id, revision.id)
                .status(ConvergenceStatus::Running)
                .build();

            let context = build_reject_approval_context(
                project.id,
                item,
                &revision,
                &[active_job],
                &[active_convergence],
            )
            .expect("reject context");

            assert!(context.has_active_job);
            assert!(context.has_active_convergence);
        }

        #[test]
        fn approval_context_rejects_cross_project_items() {
            let (_project, item, revision) = approval_context_parts();

            let other_project = ProjectBuilder::new("/tmp/ingot-context-test-other")
                .id(ProjectId::new())
                .build();
            let error = build_convergence_approval_context(
                other_project,
                item,
                revision,
                &[],
                &[],
                None,
                None,
            )
            .expect_err("cross-project item should fail");
            assert!(matches!(error, UseCaseError::ItemNotFound));
        }

        #[test]
        fn reject_context_rejects_cross_project_items() {
            let (_project, item, revision) = approval_context_parts();

            let error = build_reject_approval_context(ProjectId::new(), item, &revision, &[], &[])
                .expect_err("cross-project item should fail");
            assert!(matches!(error, UseCaseError::ItemNotFound));
        }
    }
}

pub use context::{build_convergence_approval_context, build_reject_approval_context};

mod finalization {
    use chrono::Utc;
    use ingot_domain::activity::{Activity, ActivityEventType, ActivitySubject};
    use ingot_domain::convergence::{Convergence, FinalizedCheckoutAdoption};
    use ingot_domain::convergence_queue::{ConvergenceQueueEntry, ConvergenceQueueEntryStatus};
    use ingot_domain::finding::Finding;
    use ingot_domain::git_operation::{
        GitOperation, GitOperationEntityRef, GitOperationStatus, OperationPayload,
    };
    use ingot_domain::ids::{ActivityId, ConvergenceId, ProjectId};
    use ingot_domain::item::{ApprovalState, ResolutionSource};
    use ingot_domain::job::Job;
    use ingot_domain::ports::{
        ActivityRepository, ConvergenceRepository, FinalizationCheckoutAdoptionSucceededMutation,
        FinalizationMutation, FinalizationRepository, FinalizationTargetRefAdvancedMutation,
        GitOperationRepository, RepositoryError, WorkspaceRepository,
    };
    use ingot_domain::project::Project;
    use ingot_domain::revision::{ApprovalPolicy, ItemRevision};
    use ingot_domain::workspace::WorkspaceStatus;
    use ingot_workflow::{Evaluator, NamedRecommendedAction, RecommendedAction};
    use std::path::PathBuf;
    use tracing::warn;

    use crate::UseCaseError;

    use super::types::{
        CheckoutFinalizationReadiness, FinalizePreparedTrigger, FinalizeTargetRefResult,
        PreparedConvergenceFinalizePort,
    };

    #[must_use]
    pub fn should_prepare_convergence(
        item: &ingot_domain::item::Item,
        revision: &ItemRevision,
        jobs: &[Job],
        findings: &[Finding],
        convergences: &[Convergence],
    ) -> bool {
        Evaluator::new()
            .evaluate(item, revision, jobs, findings, convergences)
            .next_recommended_action
            == RecommendedAction::named(NamedRecommendedAction::PrepareConvergence)
    }

    #[must_use]
    pub fn should_invalidate_prepared_convergence(
        item: &ingot_domain::item::Item,
        revision: &ItemRevision,
        jobs: &[Job],
        findings: &[Finding],
        convergences: &[Convergence],
    ) -> bool {
        Evaluator::new()
            .evaluate(item, revision, jobs, findings, convergences)
            .next_recommended_action
            == RecommendedAction::named(NamedRecommendedAction::InvalidatePreparedConvergence)
    }

    #[must_use]
    pub fn should_auto_finalize_prepared_convergence(
        item: &ingot_domain::item::Item,
        revision: &ItemRevision,
        jobs: &[Job],
        findings: &[Finding],
        convergences: &[Convergence],
        queue_entry: Option<&ConvergenceQueueEntry>,
    ) -> bool {
        revision.approval_policy == ApprovalPolicy::NotRequired
            && matches!(
                queue_entry,
                Some(queue_entry) if queue_entry.status == ConvergenceQueueEntryStatus::Head
            )
            && Evaluator::new()
                .evaluate(item, revision, jobs, findings, convergences)
                .next_recommended_action
                == RecommendedAction::named(NamedRecommendedAction::FinalizePreparedConvergence)
    }

    pub async fn find_or_create_finalize_operation<DB>(
        db: &DB,
        operation: &GitOperation,
    ) -> Result<GitOperation, UseCaseError>
    where
        DB: GitOperationRepository + ActivityRepository + FinalizationRepository,
    {
        let convergence_id = match &operation.entity {
            GitOperationEntityRef::Convergence(id) => *id,
            other => {
                return Err(UseCaseError::Internal(format!(
                    "expected convergence entity, got {:?}",
                    other.entity_type()
                )));
            }
        };

        if let Some(existing) = db
            .find_unresolved_finalize_for_convergence(convergence_id)
            .await
            .map_err(UseCaseError::Repository)?
        {
            return Ok(existing);
        }

        match <DB as GitOperationRepository>::create(db, operation).await {
            Ok(()) => {
                <DB as ActivityRepository>::append(
                    db,
                    &Activity {
                        id: ActivityId::new(),
                        project_id: operation.project_id,
                        event_type: ActivityEventType::GitOperationPlanned,
                        subject: ActivitySubject::GitOperation(operation.id),
                        payload: serde_json::json!({
                            "operation_kind": operation.operation_kind(),
                            "entity_id": operation.entity.entity_id_string(),
                        }),
                        created_at: Utc::now(),
                    },
                )
                .await
                .map_err(UseCaseError::Repository)?;
                Ok(operation.clone())
            }
            Err(RepositoryError::Conflict(_)) => db
                .find_unresolved_finalize_for_convergence(convergence_id)
                .await
                .map_err(UseCaseError::Repository)?
                .ok_or_else(|| {
                    UseCaseError::Internal(
                        "finalize git operation conflict without existing row".into(),
                    )
                }),
            Err(other) => Err(UseCaseError::Repository(other)),
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct FinalizedIntegrationWorkspaceCleanup {
        pub project_id: ProjectId,
        pub convergence_id: ConvergenceId,
        pub workspace_path: PathBuf,
    }

    pub async fn apply_finalization_mutation_and_load_cleanup<DB>(
        db: &DB,
        mutation: FinalizationMutation,
    ) -> Result<Option<FinalizedIntegrationWorkspaceCleanup>, UseCaseError>
    where
        DB: FinalizationRepository + ConvergenceRepository + WorkspaceRepository,
    {
        let cleanup = match &mutation {
            FinalizationMutation::TargetRefAdvanced(mutation) => {
                Some((mutation.project_id, mutation.convergence_id))
            }
            FinalizationMutation::CheckoutAdoptionSucceeded(_) => None,
        };

        db.apply_finalization_mutation(mutation)
            .await
            .map_err(UseCaseError::Repository)?;

        let Some((project_id, convergence_id)) = cleanup else {
            return Ok(None);
        };
        let convergence = match <DB as ConvergenceRepository>::get(db, convergence_id).await {
            Ok(convergence) => convergence,
            Err(error) => {
                warn!(
                    project_id = %project_id,
                    convergence_id = %convergence_id,
                    ?error,
                    "failed best-effort integration workspace cleanup lookup after committed finalization",
                );
                return Ok(None);
            }
        };
        let Some(workspace_id) = convergence.state.integration_workspace_id() else {
            return Ok(None);
        };
        let workspace = match <DB as WorkspaceRepository>::get(db, workspace_id).await {
            Ok(workspace) => workspace,
            Err(error) => {
                warn!(
                    project_id = %project_id,
                    convergence_id = %convergence_id,
                    workspace_id = %workspace_id,
                    ?error,
                    "failed best-effort integration workspace cleanup lookup after committed finalization",
                );
                return Ok(None);
            }
        };
        if workspace.state.status() != WorkspaceStatus::Abandoned {
            return Ok(None);
        }

        Ok(Some(FinalizedIntegrationWorkspaceCleanup {
            project_id,
            convergence_id,
            workspace_path: workspace.path,
        }))
    }

    pub async fn finalize_prepared_convergence<P>(
        port: &P,
        trigger: FinalizePreparedTrigger,
        project: &Project,
        item: &ingot_domain::item::Item,
        revision: &ItemRevision,
        convergence: &Convergence,
        _queue_entry: &ConvergenceQueueEntry,
    ) -> Result<(), UseCaseError>
    where
        P: PreparedConvergenceFinalizePort,
    {
        let prepared_commit_oid = convergence
            .state
            .prepared_commit_oid()
            .map(ToOwned::to_owned)
            .ok_or(UseCaseError::PreparedConvergenceMissing)?;
        let input_target_commit_oid = convergence
            .state
            .input_target_commit_oid()
            .map(ToOwned::to_owned)
            .ok_or(UseCaseError::PreparedConvergenceMissing)?;

        let planned_operation = GitOperation {
            id: ingot_domain::ids::GitOperationId::new(),
            project_id: project.id,
            entity: GitOperationEntityRef::Convergence(convergence.id),
            payload: OperationPayload::FinalizeTargetRef {
                workspace_id: convergence.state.integration_workspace_id(),
                ref_name: convergence.target_ref.clone(),
                expected_old_oid: input_target_commit_oid,
                new_oid: prepared_commit_oid.clone(),
                commit_oid: Some(prepared_commit_oid.clone()),
            },
            status: GitOperationStatus::Planned,
            created_at: Utc::now(),
            completed_at: None,
        };
        let mut operation = port
            .find_or_create_finalize_operation(&planned_operation)
            .await?;

        if port.finalize_target_ref(project, convergence).await? == FinalizeTargetRefResult::Stale {
            operation.status = GitOperationStatus::Failed;
            operation.completed_at = Some(Utc::now());
            port.update_git_operation(&operation).await?;
            return Err(UseCaseError::PreparedConvergenceStale);
        }

        if operation.status == GitOperationStatus::Planned {
            operation.status = GitOperationStatus::Applied;
            operation.completed_at = Some(Utc::now());
            port.update_git_operation(&operation).await?;
        }

        let readiness = port
            .checkout_finalization_readiness(project, item, revision, &prepared_commit_oid)
            .await;
        let initial_checkout_adoption = match &readiness {
            Ok(CheckoutFinalizationReadiness::Blocked { message }) => {
                FinalizedCheckoutAdoption::blocked(message.clone(), Utc::now())
            }
            Ok(CheckoutFinalizationReadiness::NeedsSync) => {
                FinalizedCheckoutAdoption::pending(Utc::now())
            }
            Ok(CheckoutFinalizationReadiness::Synced) => {
                FinalizedCheckoutAdoption::synced(Utc::now())
            }
            Err(_) => FinalizedCheckoutAdoption::pending(Utc::now()),
        };

        port.apply_finalization_mutation(FinalizationMutation::TargetRefAdvanced(
            FinalizationTargetRefAdvancedMutation {
                project_id: project.id,
                item_id: item.id,
                expected_item_revision_id: revision.id,
                convergence_id: convergence.id,
                git_operation_id: operation.id,
                final_target_commit_oid: prepared_commit_oid.clone(),
                checkout_adoption: initial_checkout_adoption,
            },
        ))
        .await?;

        let readiness = readiness?;
        let checkout_adopted = match readiness {
            CheckoutFinalizationReadiness::Blocked { .. } => false,
            CheckoutFinalizationReadiness::NeedsSync => {
                if port
                    .sync_checkout_to_prepared_commit(project, revision, &prepared_commit_oid)
                    .await
                    .is_ok()
                {
                    true
                } else {
                    if let Ok(CheckoutFinalizationReadiness::Blocked { message }) = port
                        .checkout_finalization_readiness(
                            project,
                            item,
                            revision,
                            &prepared_commit_oid,
                        )
                        .await
                    {
                        let blocked = FinalizedCheckoutAdoption::blocked(message, Utc::now());
                        port.apply_finalization_mutation(FinalizationMutation::TargetRefAdvanced(
                            FinalizationTargetRefAdvancedMutation {
                                project_id: project.id,
                                item_id: item.id,
                                expected_item_revision_id: revision.id,
                                convergence_id: convergence.id,
                                git_operation_id: operation.id,
                                final_target_commit_oid: prepared_commit_oid.clone(),
                                checkout_adoption: blocked,
                            },
                        ))
                        .await?;
                    }
                    false
                }
            }
            CheckoutFinalizationReadiness::Synced => true,
        };

        if checkout_adopted {
            let (resolution_source, approval_state) = match trigger {
                FinalizePreparedTrigger::ApprovalCommand => {
                    (ResolutionSource::ApprovalCommand, ApprovalState::Approved)
                }
                FinalizePreparedTrigger::SystemCommand => {
                    (ResolutionSource::SystemCommand, ApprovalState::NotRequired)
                }
            };
            port.apply_finalization_mutation(FinalizationMutation::CheckoutAdoptionSucceeded(
                FinalizationCheckoutAdoptionSucceededMutation {
                    project_id: project.id,
                    item_id: item.id,
                    expected_item_revision_id: revision.id,
                    convergence_id: convergence.id,
                    git_operation_id: operation.id,
                    resolution_source,
                    approval_state,
                    synced_at: Utc::now(),
                },
            ))
            .await?;
        }

        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use std::sync::Mutex;

        use ingot_domain::commit_oid::CommitOid;
        use ingot_domain::ids::{GitOperationId, ItemId, ItemRevisionId, WorkspaceId};
        use ingot_domain::workspace::Workspace;

        use super::*;

        struct CleanupLookupFailsDb {
            applied: Mutex<bool>,
        }

        impl CleanupLookupFailsDb {
            fn new() -> Self {
                Self {
                    applied: Mutex::new(false),
                }
            }

            fn applied(&self) -> bool {
                *self.applied.lock().expect("applied lock")
            }
        }

        impl FinalizationRepository for CleanupLookupFailsDb {
            async fn apply_finalization_mutation(
                &self,
                _mutation: FinalizationMutation,
            ) -> Result<(), RepositoryError> {
                *self.applied.lock().expect("applied lock") = true;
                Ok(())
            }
        }

        impl ConvergenceRepository for CleanupLookupFailsDb {
            async fn list_by_revision(
                &self,
                _revision_id: ItemRevisionId,
            ) -> Result<Vec<Convergence>, RepositoryError> {
                unreachable!("cleanup helper only loads convergence by id")
            }

            async fn get(&self, _id: ConvergenceId) -> Result<Convergence, RepositoryError> {
                Err(RepositoryError::Database(Box::new(std::io::Error::other(
                    "lookup failed",
                ))))
            }

            async fn create(&self, _convergence: &Convergence) -> Result<(), RepositoryError> {
                unreachable!("cleanup helper does not create convergence")
            }

            async fn update(&self, _convergence: &Convergence) -> Result<(), RepositoryError> {
                unreachable!("cleanup helper does not update convergence")
            }

            async fn find_active_for_revision(
                &self,
                _revision_id: ItemRevisionId,
            ) -> Result<Option<Convergence>, RepositoryError> {
                unreachable!("cleanup helper does not find active convergence")
            }

            async fn find_prepared_for_revision(
                &self,
                _revision_id: ItemRevisionId,
            ) -> Result<Option<Convergence>, RepositoryError> {
                unreachable!("cleanup helper does not find prepared convergence")
            }

            async fn list_by_item(
                &self,
                _item_id: ItemId,
            ) -> Result<Vec<Convergence>, RepositoryError> {
                unreachable!("cleanup helper does not list convergence by item")
            }

            async fn list_active(&self) -> Result<Vec<Convergence>, RepositoryError> {
                unreachable!("cleanup helper does not list active convergences")
            }
        }

        impl WorkspaceRepository for CleanupLookupFailsDb {
            async fn list_by_project(
                &self,
                _project_id: ProjectId,
            ) -> Result<Vec<Workspace>, RepositoryError> {
                unreachable!("cleanup helper does not list workspaces by project")
            }

            async fn get(&self, _id: WorkspaceId) -> Result<Workspace, RepositoryError> {
                unreachable!("convergence lookup fails before workspace lookup")
            }

            async fn create(&self, _workspace: &Workspace) -> Result<(), RepositoryError> {
                unreachable!("cleanup helper does not create workspace")
            }

            async fn update(&self, _workspace: &Workspace) -> Result<(), RepositoryError> {
                unreachable!("cleanup helper does not update workspace")
            }

            async fn find_authoring_for_revision(
                &self,
                _revision_id: ItemRevisionId,
            ) -> Result<Option<Workspace>, RepositoryError> {
                unreachable!("cleanup helper does not find authoring workspace")
            }

            async fn list_by_item(
                &self,
                _item_id: ItemId,
            ) -> Result<Vec<Workspace>, RepositoryError> {
                unreachable!("cleanup helper does not list workspaces by item")
            }

            async fn delete(&self, _id: WorkspaceId) -> Result<(), RepositoryError> {
                unreachable!("cleanup helper does not delete workspace")
            }
        }

        fn target_ref_advanced_mutation() -> FinalizationMutation {
            FinalizationMutation::TargetRefAdvanced(FinalizationTargetRefAdvancedMutation {
                project_id: ProjectId::new(),
                item_id: ItemId::new(),
                expected_item_revision_id: ItemRevisionId::new(),
                convergence_id: ConvergenceId::new(),
                git_operation_id: GitOperationId::new(),
                final_target_commit_oid: CommitOid::new("abc123"),
                checkout_adoption: FinalizedCheckoutAdoption::pending(Utc::now()),
            })
        }

        #[tokio::test]
        async fn cleanup_lookup_failure_after_committed_finalization_is_best_effort() {
            let db = CleanupLookupFailsDb::new();

            let cleanup =
                apply_finalization_mutation_and_load_cleanup(&db, target_ref_advanced_mutation())
                    .await
                    .expect("cleanup lookup failure should not fail committed finalization");

            assert!(cleanup.is_none());
            assert!(
                db.applied(),
                "finalization mutation should still be applied"
            );
        }
    }
}

pub use finalization::{
    FinalizedIntegrationWorkspaceCleanup, apply_finalization_mutation_and_load_cleanup,
    finalize_prepared_convergence, find_or_create_finalize_operation,
    should_auto_finalize_prepared_convergence, should_invalidate_prepared_convergence,
    should_prepare_convergence,
};
