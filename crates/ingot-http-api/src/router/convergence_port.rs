use ingot_domain::activity::Activity;
use ingot_domain::commit_oid::CommitOid;
use ingot_domain::convergence::Convergence;
use ingot_domain::convergence_queue::ConvergenceQueueEntry;
use ingot_domain::git_operation::GitOperation;
use ingot_domain::ids::{ItemId, ProjectId};
use ingot_domain::item::Item;
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
    CheckoutFinalizationReadiness, ConvergenceCommandPort, ConvergenceQueuePrepareContext,
    FinalizeTargetRefResult, PreparedConvergenceFinalizePort,
    apply_finalization_mutation_and_load_cleanup, build_convergence_approval_context,
    build_reject_approval_context,
};
use tracing::warn;

use super::app::{AppState, teardown_revision_lane_state};
use super::support::errors::{repo_to_item_usecase, repo_to_project_usecase};

#[derive(Clone)]
pub(super) struct HttpConvergencePort {
    pub(super) state: AppState,
}

impl HttpConvergencePort {
    pub(super) fn new(state: &AppState) -> Self {
        Self {
            state: state.clone(),
        }
    }
}

impl ConvergenceCommandPort for HttpConvergencePort {
    fn load_queue_prepare_context(
        &self,
        project_id: ProjectId,
        item_id: ItemId,
    ) -> impl std::future::Future<Output = Result<ConvergenceQueuePrepareContext, UseCaseError>> + Send
    {
        let state = self.state.clone();
        async move {
            let project = state
                .db
                .get_project(project_id)
                .await
                .map_err(repo_to_project_usecase)?;
            let item = state
                .db
                .get_item(item_id)
                .await
                .map_err(repo_to_item_usecase)?;
            let ItemRuntimeSnapshot {
                current_revision,
                jobs,
                findings,
                convergences,
            } = load_item_runtime_snapshot(&state.db, &state.infra(), project.id, &item).await?;
            let active_queue_entry = state
                .db
                .find_active_queue_entry_for_revision(current_revision.id)
                .await
                .map_err(UseCaseError::Repository)?;
            let lane_head = state
                .db
                .find_queue_head(project.id, &current_revision.target_ref)
                .await
                .map_err(UseCaseError::Repository)?;

            Ok(ConvergenceQueuePrepareContext {
                project,
                item,
                revision: current_revision,
                jobs,
                findings,
                convergences,
                active_queue_entry,
                lane_head,
            })
        }
    }

    fn create_queue_entry(
        &self,
        queue_entry: &ConvergenceQueueEntry,
    ) -> impl std::future::Future<Output = Result<(), UseCaseError>> + Send {
        let db = self.state.db.clone();
        let queue_entry = queue_entry.clone();
        async move {
            db.create_queue_entry(&queue_entry)
                .await
                .map_err(UseCaseError::Repository)
        }
    }

    fn update_queue_entry(
        &self,
        queue_entry: &ConvergenceQueueEntry,
    ) -> impl std::future::Future<Output = Result<(), UseCaseError>> + Send {
        let db = self.state.db.clone();
        let queue_entry = queue_entry.clone();
        async move {
            db.update_queue_entry(&queue_entry)
                .await
                .map_err(UseCaseError::Repository)
        }
    }

    fn append_activity(
        &self,
        activity: &Activity,
    ) -> impl std::future::Future<Output = Result<(), UseCaseError>> + Send {
        let state = self.state.clone();
        let activity = activity.clone();
        async move {
            state
                .db
                .append_activity(&activity)
                .await
                .map_err(UseCaseError::Repository)?;
            state.ui_events.publish_entity_changed(
                activity.project_id,
                activity.event_type,
                activity.subject.clone(),
                activity.payload.clone(),
            );
            Ok(())
        }
    }

    fn load_approval_context(
        &self,
        project_id: ProjectId,
        item_id: ItemId,
    ) -> impl std::future::Future<
        Output = Result<ingot_usecases::convergence::ConvergenceApprovalContext, UseCaseError>,
    > + Send {
        let state = self.state.clone();
        async move {
            let project = state
                .db
                .get_project(project_id)
                .await
                .map_err(repo_to_project_usecase)?;
            let item = state
                .db
                .get_item(item_id)
                .await
                .map_err(repo_to_item_usecase)?;
            if item.project_id != project_id {
                return Err(UseCaseError::ItemNotFound);
            }
            let revision = state
                .db
                .get_revision(item.current_revision_id)
                .await
                .map_err(UseCaseError::Repository)?;
            let jobs = state
                .db
                .list_jobs_by_item(item.id)
                .await
                .map_err(UseCaseError::Repository)?;
            let mut convergences = state
                .db
                .list_convergences_by_item(item.id)
                .await
                .map_err(UseCaseError::Repository)?;
            hydrate_convergence_validity(&state.infra(), project.id, &mut convergences).await?;
            let queue_entry = state
                .db
                .find_active_queue_entry_for_revision(revision.id)
                .await
                .map_err(UseCaseError::Repository)?;
            let resolved_target_oid = state
                .infra()
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
    }

    fn update_item(
        &self,
        item: &Item,
    ) -> impl std::future::Future<Output = Result<(), UseCaseError>> + Send {
        let db = self.state.db.clone();
        let item = item.clone();
        async move {
            db.update_item(&item)
                .await
                .map_err(UseCaseError::Repository)
        }
    }

    fn load_reject_approval_context(
        &self,
        project_id: ProjectId,
        item_id: ItemId,
    ) -> impl std::future::Future<
        Output = Result<ingot_usecases::convergence::RejectApprovalContext, UseCaseError>,
    > + Send {
        let state = self.state.clone();
        async move {
            let item = state
                .db
                .get_item(item_id)
                .await
                .map_err(repo_to_item_usecase)?;
            if item.project_id != project_id {
                return Err(UseCaseError::ItemNotFound);
            }
            let revision = state
                .db
                .get_revision(item.current_revision_id)
                .await
                .map_err(UseCaseError::Repository)?;
            let jobs = state
                .db
                .list_jobs_by_item(item.id)
                .await
                .map_err(UseCaseError::Repository)?;
            let convergences = state
                .db
                .list_convergences_by_item(item.id)
                .await
                .map_err(UseCaseError::Repository)?;

            build_reject_approval_context(project_id, item, &revision, &jobs, &convergences)
        }
    }

    fn teardown_reject_approval(
        &self,
        project_id: ProjectId,
        item_id: ItemId,
    ) -> impl std::future::Future<
        Output = Result<ingot_usecases::convergence::RejectApprovalTeardown, UseCaseError>,
    > + Send {
        let state = self.state.clone();
        async move {
            let project = state
                .db
                .get_project(project_id)
                .await
                .map_err(UseCaseError::Repository)?;
            let item = state
                .db
                .get_item(item_id)
                .await
                .map_err(UseCaseError::Repository)?;
            let revision = state
                .db
                .get_revision(item.current_revision_id)
                .await
                .map_err(UseCaseError::Repository)?;
            let teardown =
                teardown_revision_lane_state(&state, &project, item.id, &revision).await?;
            Ok(ingot_usecases::convergence::RejectApprovalTeardown {
                has_cancelled_convergence: teardown.has_cancelled_convergence(),
                has_cancelled_queue_entry: teardown.has_cancelled_queue_entry(),
                first_cancelled_convergence_id: teardown
                    .first_cancelled_convergence_id()
                    .map(ToOwned::to_owned),
                first_cancelled_queue_entry_id: teardown
                    .first_cancelled_queue_entry_id()
                    .map(ToOwned::to_owned),
            })
        }
    }

    fn apply_rejected_approval(
        &self,
        item: &Item,
        next_revision: &ItemRevision,
    ) -> impl std::future::Future<Output = Result<(), UseCaseError>> + Send {
        let db = self.state.db.clone();
        let item = item.clone();
        let next_revision = next_revision.clone();
        async move {
            db.create_revision(&next_revision)
                .await
                .map_err(UseCaseError::Repository)?;
            db.update_item(&item)
                .await
                .map_err(UseCaseError::Repository)
        }
    }
}

impl PreparedConvergenceFinalizePort for HttpConvergencePort {
    fn find_or_create_finalize_operation(
        &self,
        operation: &GitOperation,
    ) -> impl std::future::Future<Output = Result<GitOperation, UseCaseError>> + Send {
        let db = self.state.db.clone();
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
        let state = self.state.clone();
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

            match state
                .infra()
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
        let state = self.state.clone();
        let project = project.clone();
        let revision = revision.clone();
        let prepared_commit_oid = prepared_commit_oid.clone();
        async move {
            match state
                .infra()
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
        let state = self.state.clone();
        let project = project.clone();
        let revision = revision.clone();
        let prepared_commit_oid = prepared_commit_oid.clone();
        async move {
            state
                .infra()
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
        let db = self.state.db.clone();
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
        let state = self.state.clone();
        async move {
            let cleanup = apply_finalization_mutation_and_load_cleanup(&state.db, mutation).await?;
            if let Some(cleanup) = cleanup {
                if let Err(error) = state
                    .infra()
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
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::Utc;
    use ingot_domain::ids::{ItemId, ItemRevisionId, ProjectId};
    use ingot_domain::item::ApprovalState;
    use ingot_domain::test_support::{ItemBuilder, ProjectBuilder, RevisionBuilder};
    use ingot_test_support::git::{
        git_output as support_git_output, temp_git_repo as support_temp_git_repo, unique_temp_path,
    };
    use ingot_usecases::convergence::ConvergenceCommandPort;

    use super::*;
    use crate::router::test_helpers::test_app_state;

    fn temp_git_repo() -> PathBuf {
        support_temp_git_repo("ingot-http-api")
    }

    fn git_output(path: &std::path::Path, args: &[&str]) -> String {
        support_git_output(path, args)
    }

    #[tokio::test]
    async fn convergence_port_maps_missing_project_to_project_not_found() {
        let state = test_app_state().await;
        let error = HttpConvergencePort::new(&state)
            .load_queue_prepare_context(ProjectId::new(), ItemId::new())
            .await
            .expect_err("missing project should fail");

        assert!(matches!(error, UseCaseError::ProjectNotFound));
    }

    #[tokio::test]
    async fn convergence_port_rejects_cross_project_approval_context() {
        let state = test_app_state().await;
        let repo_b = temp_git_repo();
        let missing_repo = unique_temp_path("ingot-http-api-missing-repo");
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
        state
            .db
            .create_project(&project_a)
            .await
            .expect("project a");
        state
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
        state
            .db
            .create_item_with_revision(&item, &revision)
            .await
            .expect("item b");

        let error = HttpConvergencePort::new(&state)
            .load_approval_context(project_a.id, item.id)
            .await
            .expect_err("cross-project item should fail");

        assert!(matches!(error, UseCaseError::ItemNotFound));
    }
}
