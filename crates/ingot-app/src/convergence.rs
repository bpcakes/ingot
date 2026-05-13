use chrono::Utc;
use ingot_domain::activity::{Activity, ActivityEventType, ActivitySubject};
use ingot_domain::commit_oid::CommitOid;
use ingot_domain::convergence::Convergence;
use ingot_domain::convergence_queue::ConvergenceQueueEntryStatus;
use ingot_domain::git_operation::GitOperation;
use ingot_domain::ids::{ActivityId, ItemId, ProjectId};
use ingot_domain::item::{ApprovalState, Item};
use ingot_domain::ports::FinalizationMutation;
use ingot_domain::project::Project;
use ingot_domain::revision::ItemRevision;
use ingot_git::commands::FinalizeTargetRefOutcome;
use ingot_git::project_repo::CheckoutFinalizationStatus;
use ingot_usecases::UseCaseError;
use ingot_usecases::application::{
    ItemRuntimeSnapshot, hydrate_convergence_validity, load_item_runtime_snapshot,
};
use ingot_usecases::convergence::{
    ApprovalFinalizeReadiness, CheckoutFinalizationReadiness, ConvergenceApprovalContext,
    FinalizePreparedTrigger, FinalizeTargetRefResult, PreparedConvergenceFinalizePort,
    apply_finalization_mutation_and_load_cleanup, build_convergence_approval_context,
    build_convergence_queue_entry, build_reject_approval_context, finalize_prepared_convergence,
    should_prepare_convergence,
};
use ingot_usecases::item::approval_state_for_policy;
use tracing::warn;

use crate::errors::{repo_to_item_usecase, repo_to_project_usecase};
use crate::{
    ApplicationServices, RevisionLaneTeardown, plan_revision_lane_state,
    refresh_and_cleanup_revision_lane_state,
};

/// Public summary of the lane state cancelled by approval rejection.
///
/// The lower-level teardown result also tracks job, workspace, and git-operation
/// ids for internal cleanup/accounting. The approval API intentionally exposes
/// only the convergence and queue-entry values its callers record in activity.
#[derive(Debug, Clone, Default)]
pub struct RejectApprovalTeardown {
    pub has_cancelled_convergence: bool,
    pub has_cancelled_queue_entry: bool,
    pub first_cancelled_convergence_id: Option<String>,
    pub first_cancelled_queue_entry_id: Option<String>,
}

impl RejectApprovalTeardown {
    pub(crate) fn from_revision_lane_teardown(teardown: &RevisionLaneTeardown) -> Self {
        Self {
            has_cancelled_convergence: teardown.has_cancelled_convergence(),
            has_cancelled_queue_entry: teardown.has_cancelled_queue_entry(),
            first_cancelled_convergence_id: teardown
                .first_cancelled_convergence_id()
                .map(ToOwned::to_owned),
            first_cancelled_queue_entry_id: teardown
                .first_cancelled_queue_entry_id()
                .map(ToOwned::to_owned),
        }
    }
}

#[derive(Clone)]
pub(crate) struct ApplicationFinalizePort {
    services: ApplicationServices,
}

impl ApplicationFinalizePort {
    pub(crate) fn new(services: &ApplicationServices) -> Self {
        Self {
            services: services.clone(),
        }
    }
}

pub(crate) async fn queue_prepare_convergence(
    services: &ApplicationServices,
    project_id: ProjectId,
    item_id: ItemId,
) -> Result<(), UseCaseError> {
    let project = services
        .db
        .get_project(project_id)
        .await
        .map_err(repo_to_project_usecase)?;
    let item = services
        .db
        .get_item(item_id)
        .await
        .map_err(repo_to_item_usecase)?;
    if item.project_id != project_id {
        return Err(UseCaseError::ItemNotFound);
    }

    let ItemRuntimeSnapshot {
        current_revision,
        jobs,
        findings,
        convergences,
    } = load_item_runtime_snapshot(&services.db, services.runtime_infra(), project.id, &item)
        .await?;
    let active_queue_entry = services
        .db
        .find_active_queue_entry_for_revision(current_revision.id)
        .await
        .map_err(UseCaseError::Repository)?;
    let lane_head = services
        .db
        .find_queue_head(project.id, &current_revision.target_ref)
        .await
        .map_err(UseCaseError::Repository)?;

    if active_queue_entry.is_none()
        && !should_prepare_convergence(&item, &current_revision, &jobs, &findings, &convergences)
    {
        return Err(UseCaseError::ConvergenceNotPreparable);
    }

    let mut queue_entry = if let Some(queue_entry) = active_queue_entry {
        queue_entry
    } else {
        let now = Utc::now();
        let queue_entry = build_convergence_queue_entry(
            project.id,
            item.id,
            &current_revision,
            lane_head.is_some(),
            now,
        );
        services
            .db
            .create_queue_entry(&queue_entry)
            .await
            .map_err(UseCaseError::Repository)?;
        append_activity(
            services,
            Activity {
                id: ActivityId::new(),
                project_id: project.id,
                event_type: ActivityEventType::ConvergenceQueued,
                subject: ActivitySubject::QueueEntry(queue_entry.id),
                payload: serde_json::json!({
                    "item_id": item.id,
                    "target_ref": current_revision.target_ref,
                }),
                created_at: now,
            },
        )
        .await?;
        queue_entry
    };

    if queue_entry.status == ConvergenceQueueEntryStatus::Queued && lane_head.is_none() {
        queue_entry.status = ConvergenceQueueEntryStatus::Head;
        queue_entry.head_acquired_at = Some(Utc::now());
        queue_entry.updated_at = Utc::now();
        services
            .db
            .update_queue_entry(&queue_entry)
            .await
            .map_err(UseCaseError::Repository)?;
        append_activity(
            services,
            Activity {
                id: ActivityId::new(),
                project_id: project.id,
                event_type: ActivityEventType::ConvergenceLaneAcquired,
                subject: ActivitySubject::QueueEntry(queue_entry.id),
                payload: serde_json::json!({
                    "item_id": item.id,
                    "target_ref": current_revision.target_ref,
                }),
                created_at: Utc::now(),
            },
        )
        .await?;
    }

    Ok(())
}

pub(crate) async fn approve_item(
    services: &ApplicationServices,
    project_id: ProjectId,
    item_id: ItemId,
) -> Result<(), UseCaseError> {
    let ConvergenceApprovalContext {
        project,
        item,
        revision,
        has_active_job,
        has_active_convergence,
        finalize_readiness,
    } = load_approval_context(services, project_id, item_id).await?;

    if item.approval_state != ApprovalState::Pending {
        return Err(UseCaseError::ApprovalNotPending);
    }
    if has_active_job {
        return Err(UseCaseError::ActiveJobExists);
    }
    if has_active_convergence {
        return Err(UseCaseError::ActiveConvergenceExists);
    }
    let (convergence, queue_entry) = match finalize_readiness {
        ApprovalFinalizeReadiness::MissingPreparedConvergence => {
            return Err(UseCaseError::PreparedConvergenceMissing);
        }
        ApprovalFinalizeReadiness::PreparedConvergenceStale => {
            return Err(UseCaseError::PreparedConvergenceStale);
        }
        ApprovalFinalizeReadiness::ConvergenceNotQueued => {
            return Err(UseCaseError::ConvergenceNotQueued);
        }
        ApprovalFinalizeReadiness::ConvergenceNotLaneHead => {
            return Err(UseCaseError::ConvergenceNotLaneHead);
        }
        ApprovalFinalizeReadiness::Ready {
            convergence,
            queue_entry,
        } => (convergence, queue_entry),
    };

    finalize_prepared_convergence(
        &ApplicationFinalizePort::new(services),
        FinalizePreparedTrigger::ApprovalCommand,
        &project,
        &item,
        &revision,
        &convergence,
        &queue_entry,
    )
    .await?;

    append_activity(
        services,
        Activity {
            id: ActivityId::new(),
            project_id,
            event_type: ActivityEventType::ApprovalApproved,
            subject: ActivitySubject::Item(item.id),
            payload: serde_json::json!({
                "convergence_id": convergence.id,
                "queue_entry_id": queue_entry.id,
            }),
            created_at: Utc::now(),
        },
    )
    .await?;
    Ok(())
}

pub(crate) async fn reject_item_approval(
    services: &ApplicationServices,
    project_id: ProjectId,
    item_id: ItemId,
    next_revision: &ItemRevision,
) -> Result<RejectApprovalTeardown, UseCaseError> {
    let project = services
        .db
        .get_project(project_id)
        .await
        .map_err(repo_to_project_usecase)?;
    let item = services
        .db
        .get_item(item_id)
        .await
        .map_err(repo_to_item_usecase)?;
    if item.project_id != project_id {
        return Err(UseCaseError::ItemNotFound);
    }
    let revision = services
        .db
        .get_revision(item.current_revision_id)
        .await
        .map_err(UseCaseError::Repository)?;
    let jobs = services
        .db
        .list_jobs_by_item(item.id)
        .await
        .map_err(UseCaseError::Repository)?;
    let convergences = services
        .db
        .list_convergences_by_item(item.id)
        .await
        .map_err(UseCaseError::Repository)?;
    let mut context =
        build_reject_approval_context(project_id, item, &revision, &jobs, &convergences)?;

    if context.item.approval_state != ApprovalState::Pending {
        return Err(UseCaseError::ApprovalNotPending);
    }
    if context.has_active_job {
        return Err(UseCaseError::ActiveJobExists);
    }
    if context.has_active_convergence {
        return Err(UseCaseError::ActiveConvergenceExists);
    }

    let teardown_plan =
        plan_revision_lane_state(services, project.id, context.item.id, &revision).await?;
    let teardown = RejectApprovalTeardown::from_revision_lane_teardown(&teardown_plan.teardown);
    if !teardown.has_cancelled_convergence {
        return Err(UseCaseError::PreparedConvergenceMissing);
    }

    context.item.current_revision_id = next_revision.id;
    context.item.approval_state = approval_state_for_policy(next_revision.approval_policy);
    context.item.escalation = ingot_domain::item::Escalation::None;
    context.item.updated_at = Utc::now();
    services
        .db
        .apply_revision_lane_teardown_and_create_revision(
            teardown_plan.mutation,
            next_revision,
            &context.item,
        )
        .await
        .map_err(UseCaseError::Repository)?;
    refresh_and_cleanup_revision_lane_state(
        services,
        &project,
        context.item.id,
        &revision,
        &teardown_plan.integration_workspace_ids,
    )
    .await?;

    Ok(teardown)
}

async fn load_approval_context(
    services: &ApplicationServices,
    project_id: ProjectId,
    item_id: ItemId,
) -> Result<ConvergenceApprovalContext, UseCaseError> {
    let project = services
        .db
        .get_project(project_id)
        .await
        .map_err(repo_to_project_usecase)?;
    let item = services
        .db
        .get_item(item_id)
        .await
        .map_err(repo_to_item_usecase)?;
    if item.project_id != project_id {
        return Err(UseCaseError::ItemNotFound);
    }
    let revision = services
        .db
        .get_revision(item.current_revision_id)
        .await
        .map_err(UseCaseError::Repository)?;
    let jobs = services
        .db
        .list_jobs_by_item(item.id)
        .await
        .map_err(UseCaseError::Repository)?;
    let mut convergences = services
        .db
        .list_convergences_by_item(item.id)
        .await
        .map_err(UseCaseError::Repository)?;
    hydrate_convergence_validity(services.runtime_infra(), project.id, &mut convergences).await?;
    let queue_entry = services
        .db
        .find_active_queue_entry_for_revision(revision.id)
        .await
        .map_err(UseCaseError::Repository)?;
    let resolved_target_oid = services
        .runtime_infra()
        .resolve_project_ref_oid(project.id, &revision.target_ref)
        .await?;

    build_convergence_approval_context(
        project,
        item,
        revision,
        &jobs,
        &convergences,
        queue_entry,
        resolved_target_oid.as_ref(),
    )
}

async fn append_activity(
    services: &ApplicationServices,
    activity: Activity,
) -> Result<(), UseCaseError> {
    services
        .db
        .append_activity(&activity)
        .await
        .map_err(UseCaseError::Repository)?;
    services.ui_events.publish_entity_changed(
        activity.project_id,
        activity.event_type,
        activity.subject,
        activity.payload,
    );
    Ok(())
}

impl PreparedConvergenceFinalizePort for ApplicationFinalizePort {
    fn find_or_create_finalize_operation(
        &self,
        operation: &GitOperation,
    ) -> impl std::future::Future<Output = Result<GitOperation, UseCaseError>> + Send {
        let db = self.services.db.clone();
        let operation = operation.clone();
        async move {
            ingot_usecases::convergence::find_or_create_finalize_operation(&db, &operation).await
        }
    }

    fn finalize_target_ref(
        &self,
        project: &Project,
        convergence: &Convergence,
    ) -> impl std::future::Future<Output = Result<FinalizeTargetRefResult, UseCaseError>> + Send
    {
        let services = self.services.clone();
        let project = project.clone();
        let convergence = convergence.clone();
        async move {
            let prepared_commit_oid = convergence
                .state
                .prepared_commit_oid()
                .ok_or(UseCaseError::PreparedConvergenceMissing)?;
            let input_target_commit_oid = convergence
                .state
                .input_target_commit_oid()
                .ok_or(UseCaseError::PreparedConvergenceMissing)?;

            match services
                .runtime_infra()
                .finalize_target_ref(
                    project.id,
                    &convergence.target_ref,
                    prepared_commit_oid,
                    input_target_commit_oid,
                )
                .await?
            {
                FinalizeTargetRefOutcome::AlreadyFinalized => {
                    Ok(FinalizeTargetRefResult::AlreadyFinalized)
                }
                FinalizeTargetRefOutcome::UpdatedNow => Ok(FinalizeTargetRefResult::UpdatedNow),
                FinalizeTargetRefOutcome::Stale => Ok(FinalizeTargetRefResult::Stale),
            }
        }
    }

    fn checkout_finalization_readiness(
        &self,
        project: &Project,
        _item: &Item,
        revision: &ItemRevision,
        prepared_commit_oid: &CommitOid,
    ) -> impl std::future::Future<Output = Result<CheckoutFinalizationReadiness, UseCaseError>> + Send
    {
        let services = self.services.clone();
        let project = project.clone();
        let revision = revision.clone();
        let prepared_commit_oid = prepared_commit_oid.clone();
        async move {
            match services
                .runtime_infra()
                .checkout_finalization_status(&project, &revision.target_ref, &prepared_commit_oid)
                .await?
            {
                CheckoutFinalizationStatus::Blocked { message, .. } => {
                    Ok(CheckoutFinalizationReadiness::Blocked { message })
                }
                CheckoutFinalizationStatus::NeedsSync => {
                    Ok(CheckoutFinalizationReadiness::NeedsSync)
                }
                CheckoutFinalizationStatus::Synced => Ok(CheckoutFinalizationReadiness::Synced),
            }
        }
    }

    fn sync_checkout_to_prepared_commit(
        &self,
        project: &Project,
        revision: &ItemRevision,
        prepared_commit_oid: &CommitOid,
    ) -> impl std::future::Future<Output = Result<(), UseCaseError>> + Send {
        let services = self.services.clone();
        let project = project.clone();
        let revision = revision.clone();
        let prepared_commit_oid = prepared_commit_oid.clone();
        async move {
            services
                .runtime_infra()
                .sync_checkout_to_prepared_commit(
                    &project,
                    &revision.target_ref,
                    &prepared_commit_oid,
                )
                .await?;
            Ok(())
        }
    }

    fn update_git_operation(
        &self,
        operation: &GitOperation,
    ) -> impl std::future::Future<Output = Result<(), UseCaseError>> + Send {
        let db = self.services.db.clone();
        let operation = operation.clone();
        async move {
            db.update_git_operation(&operation)
                .await
                .map_err(UseCaseError::Repository)
        }
    }

    fn apply_finalization_mutation(
        &self,
        mutation: FinalizationMutation,
    ) -> impl std::future::Future<Output = Result<(), UseCaseError>> + Send {
        let services = self.services.clone();
        async move {
            let cleanup =
                apply_finalization_mutation_and_load_cleanup(&services.db, mutation).await?;
            if let Some(cleanup) = cleanup
                && let Err(error) = services
                    .runtime_infra()
                    .remove_workspace_path(cleanup.project_id, &cleanup.workspace_path)
                    .await
            {
                warn!(
                    project_id = %cleanup.project_id,
                    convergence_id = %cleanup.convergence_id,
                    ?error,
                    "failed best-effort integration workspace cleanup after committed finalization",
                );
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::Utc;
    use ingot_domain::convergence_queue::ConvergenceQueueEntryStatus;
    use ingot_domain::ids::{ItemId, ItemRevisionId, ProjectId};
    use ingot_domain::item::ApprovalState;
    use ingot_domain::test_support::{
        ConvergenceQueueEntryBuilder, ItemBuilder, ProjectBuilder, RevisionBuilder,
    };
    use ingot_test_support::env::temp_state_root;
    use ingot_test_support::git::{
        git_output as support_git_output, temp_git_repo as support_temp_git_repo, unique_temp_path,
    };
    use ingot_test_support::sqlite::migrated_test_db;
    use ingot_usecases::{DispatchNotify, ProjectLocks, UiEventBus};

    use super::*;

    async fn test_services() -> ApplicationServices {
        ApplicationServices::new(
            migrated_test_db("ingot-app-test").await,
            ProjectLocks::default(),
            temp_state_root("ingot-app-state"),
            DispatchNotify::default(),
            UiEventBus::default(),
        )
    }

    fn temp_git_repo() -> PathBuf {
        support_temp_git_repo("ingot-app")
    }

    fn git_output(path: &std::path::Path, args: &[&str]) -> String {
        support_git_output(path, args)
    }

    #[tokio::test]
    async fn application_convergence_maps_missing_project_to_project_not_found() {
        let services = test_services().await;
        let error = queue_prepare_convergence(&services, ProjectId::new(), ItemId::new())
            .await
            .expect_err("missing project should fail");

        assert!(matches!(error, UseCaseError::ProjectNotFound));
    }

    #[tokio::test]
    async fn application_convergence_rejects_cross_project_queue_prepare() {
        let services = test_services().await;
        let project_a = ProjectBuilder::new(unique_temp_path("ingot-app-project-a"))
            .id(ProjectId::new())
            .name("A")
            .created_at(Utc::now())
            .build();
        let project_b = ProjectBuilder::new(unique_temp_path("ingot-app-project-b"))
            .id(ProjectId::new())
            .name("B")
            .created_at(Utc::now())
            .build();
        services
            .db
            .create_project(&project_a)
            .await
            .expect("project a");
        services
            .db
            .create_project(&project_b)
            .await
            .expect("project b");

        let item = ItemBuilder::new(project_b.id, ItemRevisionId::new()).build();
        let revision = RevisionBuilder::new(item.id)
            .id(item.current_revision_id)
            .build();
        services
            .db
            .create_item_with_revision(&item, &revision)
            .await
            .expect("item b");

        let error = queue_prepare_convergence(&services, project_a.id, item.id)
            .await
            .expect_err("cross-project item should fail");

        assert!(matches!(error, UseCaseError::ItemNotFound));
    }

    #[tokio::test]
    async fn application_convergence_rejects_cross_project_approval() {
        let services = test_services().await;
        let repo_b = temp_git_repo();
        let missing_repo = unique_temp_path("ingot-app-missing-repo");
        let project_a = ProjectBuilder::new(&missing_repo)
            .id(ProjectId::new())
            .name("A")
            .created_at(Utc::now())
            .build();
        let mut project_b = ProjectBuilder::new(&repo_b)
            .id(ProjectId::new())
            .name("B")
            .created_at(Utc::now())
            .build();
        project_b.color = "#111".into();
        services
            .db
            .create_project(&project_a)
            .await
            .expect("project a");
        services
            .db
            .create_project(&project_b)
            .await
            .expect("project b");

        let head = git_output(&repo_b, &["rev-parse", "HEAD"]);
        let item = ItemBuilder::new(project_b.id, ItemRevisionId::new())
            .approval_state(ApprovalState::Pending)
            .build();
        let revision = RevisionBuilder::new(item.id)
            .id(item.current_revision_id)
            .explicit_seed(&head)
            .build();
        services
            .db
            .create_item_with_revision(&item, &revision)
            .await
            .expect("item b");

        let error = approve_item(&services, project_a.id, item.id)
            .await
            .expect_err("cross-project item should fail");

        assert!(matches!(error, UseCaseError::ItemNotFound));
    }

    #[tokio::test]
    async fn reject_approval_missing_prepared_convergence_leaves_queue_entry_active() {
        let services = test_services().await;
        let project = ProjectBuilder::new(unique_temp_path("ingot-app-project"))
            .id(ProjectId::new())
            .name("Project")
            .created_at(Utc::now())
            .build();
        services.db.create_project(&project).await.expect("project");

        let item = ItemBuilder::new(project.id, ItemRevisionId::new())
            .approval_state(ApprovalState::Pending)
            .build();
        let revision = RevisionBuilder::new(item.id)
            .id(item.current_revision_id)
            .build();
        services
            .db
            .create_item_with_revision(&item, &revision)
            .await
            .expect("item");
        let queue_entry =
            ConvergenceQueueEntryBuilder::new(project.id, item.id, revision.id).build();
        services
            .db
            .create_queue_entry(&queue_entry)
            .await
            .expect("queue entry");

        let next_revision = RevisionBuilder::new(item.id).build();
        let error = reject_item_approval(&services, project.id, item.id, &next_revision)
            .await
            .expect_err("missing convergence should fail before teardown");

        assert!(matches!(error, UseCaseError::PreparedConvergenceMissing));
        let active_entry = services
            .db
            .find_active_queue_entry_for_revision(revision.id)
            .await
            .expect("load active queue entry")
            .expect("queue entry should remain active");
        assert_eq!(active_entry.status, ConvergenceQueueEntryStatus::Head);
    }
}
