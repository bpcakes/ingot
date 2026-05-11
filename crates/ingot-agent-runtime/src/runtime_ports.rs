use std::future::Future;

use ingot_domain::convergence::Convergence;
use ingot_domain::convergence_queue::ConvergenceQueueEntry;
use ingot_domain::git_operation::GitOperation;
use ingot_domain::ports::FinalizationMutation;
use ingot_domain::ports::ProjectMutationLockPort;
use ingot_domain::project::Project;
use ingot_domain::revision::ItemRevision;
use ingot_git::commands::{
    FinalizeTargetRefOutcome, finalize_target_ref as finalize_target_ref_in_repo,
};
use ingot_git::project_repo::{
    CheckoutFinalizationStatus, checkout_finalization_status, sync_checkout_to_commit,
};
use ingot_usecases::convergence::{
    CheckoutFinalizationReadiness, ConvergenceSystemActionPort, FinalizeTargetRefResult,
    PreparedConvergenceFinalizePort, SystemActionItemState, SystemActionProjectState,
    apply_finalization_mutation_and_load_cleanup,
};
use ingot_usecases::reconciliation::ReconciliationPort;
use ingot_usecases::{UseCaseError, UseCaseInfraError};
use ingot_workspace::WorkspaceError;
use ingot_workspace::remove_workspace;
use tracing::warn;

use crate::{JobDispatcher, RuntimeError};

#[derive(Clone)]
pub(crate) struct RuntimeConvergencePort {
    pub(crate) dispatcher: JobDispatcher,
}

#[derive(Clone)]
pub(crate) struct RuntimeFinalizePort {
    pub(crate) dispatcher: JobDispatcher,
}

#[derive(Clone)]
pub(crate) struct RuntimeReconciliationPort {
    pub(crate) dispatcher: JobDispatcher,
}

impl ConvergenceSystemActionPort for RuntimeConvergencePort {
    fn load_system_action_projects(
        &self,
    ) -> impl Future<Output = Result<Vec<SystemActionProjectState>, ingot_usecases::UseCaseError>> + Send
    {
        let dispatcher = self.dispatcher.clone();
        async move {
            let mut projects = Vec::new();
            for project in dispatcher
                .db
                .list_projects()
                .await
                .map_err(ingot_usecases::UseCaseError::Repository)?
            {
                let mut items = Vec::new();
                for item in dispatcher
                    .db
                    .list_items_by_project(project.id)
                    .await
                    .map_err(ingot_usecases::UseCaseError::Repository)?
                {
                    let revision = dispatcher
                        .db
                        .get_revision(item.current_revision_id)
                        .await
                        .map_err(ingot_usecases::UseCaseError::Repository)?;
                    let jobs = dispatcher
                        .db
                        .list_jobs_by_item(item.id)
                        .await
                        .map_err(ingot_usecases::UseCaseError::Repository)?;
                    let findings = dispatcher
                        .db
                        .list_findings_by_item(item.id)
                        .await
                        .map_err(ingot_usecases::UseCaseError::Repository)?;
                    let convergences = match dispatcher
                        .hydrate_convergences(
                            &project,
                            dispatcher
                                .db
                                .list_convergences_by_item(item.id)
                                .await
                                .map_err(ingot_usecases::UseCaseError::Repository)?,
                        )
                        .await
                    {
                        Ok(convergences) => convergences,
                        Err(error) => {
                            warn!(
                                ?error,
                                project_id = %project.id,
                                item_id = %item.id,
                                "skipping system-action item because convergence hydration failed"
                            );
                            continue;
                        }
                    };
                    let queue_entry = dispatcher
                        .db
                        .find_active_queue_entry_for_revision(revision.id)
                        .await
                        .map_err(ingot_usecases::UseCaseError::Repository)?;
                    items.push(SystemActionItemState {
                        item_id: item.id,
                        item,
                        revision,
                        jobs,
                        findings,
                        convergences,
                        queue_entry,
                    });
                }
                projects.push(SystemActionProjectState { project, items });
            }

            Ok(projects)
        }
    }

    fn promote_queue_heads(
        &self,
        project_id: ingot_domain::ids::ProjectId,
    ) -> impl Future<Output = Result<(), ingot_usecases::UseCaseError>> + Send {
        let dispatcher = self.dispatcher.clone();
        async move {
            dispatcher
                .promote_queue_heads(project_id)
                .await
                .map_err(usecase_from_runtime_error)
        }
    }

    fn prepare_queue_head_convergence(
        &self,
        project: &Project,
        state: &SystemActionItemState,
        queue_entry: &ConvergenceQueueEntry,
    ) -> impl Future<Output = Result<(), ingot_usecases::UseCaseError>> + Send {
        let dispatcher = self.dispatcher.clone();
        let project = project.clone();
        let state = state.clone();
        let queue_entry = queue_entry.clone();
        async move {
            dispatcher
                .prepare_queue_head_convergence(
                    &project,
                    &state.item,
                    &state.revision,
                    &state.jobs,
                    &state.findings,
                    &state.convergences,
                    &queue_entry,
                )
                .await
                .map_err(usecase_from_runtime_error)
        }
    }

    fn invalidate_prepared_convergence(
        &self,
        project_id: ingot_domain::ids::ProjectId,
        item_id: ingot_domain::ids::ItemId,
    ) -> impl Future<Output = Result<(), ingot_usecases::UseCaseError>> + Send {
        let dispatcher = self.dispatcher.clone();
        async move {
            dispatcher
                .invalidate_prepared_convergence(project_id, item_id)
                .await
                .map_err(usecase_from_runtime_error)
        }
    }

    fn auto_finalize_prepared_convergence(
        &self,
        project_id: ingot_domain::ids::ProjectId,
        item_id: ingot_domain::ids::ItemId,
    ) -> impl Future<Output = Result<bool, ingot_usecases::UseCaseError>> + Send {
        let dispatcher = self.dispatcher.clone();
        async move {
            dispatcher
                .auto_finalize_prepared_convergence(project_id, item_id)
                .await
                .map_err(usecase_from_runtime_error)
        }
    }

    fn auto_queue_convergence(
        &self,
        project_id: ingot_domain::ids::ProjectId,
        item_id: ingot_domain::ids::ItemId,
    ) -> impl Future<Output = Result<bool, ingot_usecases::UseCaseError>> + Send {
        let dispatcher = self.dispatcher.clone();
        async move {
            #[cfg(test)]
            dispatcher.pause_before_auto_queue_guard().await;
            let _guard = dispatcher
                .project_locks
                .acquire_project_mutation(project_id)
                .await;
            let project = dispatcher
                .db
                .get_project(project_id)
                .await
                .map_err(ingot_usecases::UseCaseError::Repository)?;
            if project.execution_mode != ingot_domain::project::ExecutionMode::Autopilot {
                return Ok(false);
            }
            dispatcher
                .auto_queue_convergence_inner(project_id, item_id, &project)
                .await
        }
    }
}

impl PreparedConvergenceFinalizePort for RuntimeFinalizePort {
    fn find_or_create_finalize_operation(
        &self,
        operation: &GitOperation,
    ) -> impl Future<Output = Result<GitOperation, ingot_usecases::UseCaseError>> + Send {
        let db = self.dispatcher.db.clone();
        let operation = operation.clone();
        async move {
            ingot_usecases::convergence::find_or_create_finalize_operation(&db, &operation).await
        }
    }

    fn finalize_target_ref(
        &self,
        project: &Project,
        convergence: &Convergence,
    ) -> impl Future<Output = Result<FinalizeTargetRefResult, ingot_usecases::UseCaseError>> + Send
    {
        let dispatcher = self.dispatcher.clone();
        let project = project.clone();
        let convergence = convergence.clone();
        async move {
            let paths = dispatcher
                .refresh_project_mirror(&project)
                .await
                .map_err(usecase_from_runtime_error)?;
            let prepared_commit_oid = convergence
                .state
                .prepared_commit_oid()
                .map(ToOwned::to_owned)
                .ok_or_else(|| {
                    ingot_usecases::UseCaseError::Internal("prepared commit missing".into())
                })?;
            let input_target_commit_oid = convergence
                .state
                .input_target_commit_oid()
                .map(ToOwned::to_owned)
                .ok_or_else(|| {
                    ingot_usecases::UseCaseError::Internal("input target commit missing".into())
                })?;
            match finalize_target_ref_in_repo(
                paths.mirror_git_dir.as_path(),
                &convergence.target_ref,
                &prepared_commit_oid,
                &input_target_commit_oid,
            )
            .await
            .map_err(|error| usecase_from_runtime_error(RuntimeError::from(error)))?
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
        _item: &ingot_domain::item::Item,
        revision: &ItemRevision,
        prepared_commit_oid: &ingot_domain::commit_oid::CommitOid,
    ) -> impl Future<Output = Result<CheckoutFinalizationReadiness, ingot_usecases::UseCaseError>> + Send
    {
        let dispatcher = self.dispatcher.clone();
        let project = project.clone();
        let revision = revision.clone();
        let prepared_commit_oid = prepared_commit_oid.clone();
        async move {
            let paths = dispatcher
                .refresh_project_mirror(&project)
                .await
                .map_err(usecase_from_runtime_error)?;
            match checkout_finalization_status(
                &project.path,
                paths.mirror_git_dir.as_path(),
                &revision.target_ref,
                &prepared_commit_oid,
            )
            .await
            .map_err(|error| usecase_from_runtime_error(RuntimeError::from(error)))?
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
        prepared_commit_oid: &ingot_domain::commit_oid::CommitOid,
    ) -> impl Future<Output = Result<(), ingot_usecases::UseCaseError>> + Send {
        let dispatcher = self.dispatcher.clone();
        let project = project.clone();
        let revision = revision.clone();
        let prepared_commit_oid = prepared_commit_oid.clone();
        async move {
            let paths = dispatcher
                .refresh_project_mirror(&project)
                .await
                .map_err(usecase_from_runtime_error)?;
            sync_checkout_to_commit(
                &project.path,
                paths.mirror_git_dir.as_path(),
                &revision.target_ref,
                &prepared_commit_oid,
            )
            .await
            .map_err(|error| usecase_from_runtime_error(RuntimeError::from(error)))?;
            Ok(())
        }
    }

    fn update_git_operation(
        &self,
        operation: &GitOperation,
    ) -> impl Future<Output = Result<(), ingot_usecases::UseCaseError>> + Send {
        let dispatcher = self.dispatcher.clone();
        let operation = operation.clone();
        async move {
            dispatcher
                .db
                .update_git_operation(&operation)
                .await
                .map_err(ingot_usecases::UseCaseError::Repository)?;
            Ok(())
        }
    }

    fn apply_finalization_mutation(
        &self,
        mutation: FinalizationMutation,
    ) -> impl Future<Output = Result<(), ingot_usecases::UseCaseError>> + Send {
        let dispatcher = self.dispatcher.clone();
        async move {
            let cleanup =
                apply_finalization_mutation_and_load_cleanup(&dispatcher.db, mutation).await?;
            if let Some(cleanup) = cleanup {
                let cleanup_result: Result<(), RuntimeError> = async {
                    let repo_path = dispatcher
                        .config
                        .state_root
                        .join("repos")
                        .join(format!("{}.git", cleanup.project_id));
                    remove_workspace(repo_path.as_path(), &cleanup.workspace_path).await?;
                    Ok(())
                }
                .await;

                if let Err(error) = cleanup_result {
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

impl ReconciliationPort for RuntimeReconciliationPort {
    fn reconcile_git_operations(
        &self,
    ) -> impl Future<Output = Result<bool, ingot_usecases::UseCaseError>> + Send {
        let dispatcher = self.dispatcher.clone();
        async move {
            dispatcher
                .reconcile_git_operations()
                .await
                .map_err(usecase_from_runtime_error)
        }
    }

    fn reconcile_active_jobs(
        &self,
    ) -> impl Future<Output = Result<bool, ingot_usecases::UseCaseError>> + Send {
        let dispatcher = self.dispatcher.clone();
        async move {
            dispatcher
                .reconcile_active_jobs()
                .await
                .map_err(usecase_from_runtime_error)
        }
    }

    fn reconcile_active_convergences(
        &self,
    ) -> impl Future<Output = Result<bool, ingot_usecases::UseCaseError>> + Send {
        let dispatcher = self.dispatcher.clone();
        async move {
            dispatcher
                .reconcile_active_convergences()
                .await
                .map_err(usecase_from_runtime_error)
        }
    }

    fn reconcile_workspace_retention(
        &self,
    ) -> impl Future<Output = Result<bool, ingot_usecases::UseCaseError>> + Send {
        let dispatcher = self.dispatcher.clone();
        async move {
            dispatcher
                .reconcile_workspace_retention()
                .await
                .map_err(usecase_from_runtime_error)
        }
    }
}

pub(crate) fn usecase_to_runtime_error(error: UseCaseError) -> RuntimeError {
    match error {
        UseCaseError::Repository(error) => RuntimeError::Repository(error),
        other => RuntimeError::UseCase(other),
    }
}

pub(crate) fn usecase_from_runtime_error(error: RuntimeError) -> UseCaseError {
    match error {
        RuntimeError::UseCase(error) => error,
        RuntimeError::Repository(error) => UseCaseError::Repository(error),
        RuntimeError::Git(error) => UseCaseInfraError::git(error).into(),
        RuntimeError::Workspace(error) => workspace_to_usecase_error(error),
        RuntimeError::Io(error) => UseCaseInfraError::io(error).into(),
        RuntimeError::Json(error) => UseCaseInfraError::serialization(error).into(),
        RuntimeError::InvalidState(message) => UseCaseError::Internal(message),
    }
}

fn workspace_to_usecase_error(error: WorkspaceError) -> UseCaseError {
    match error {
        error @ WorkspaceError::Busy => UseCaseInfraError::workspace_busy(error).into(),
        error @ WorkspaceError::MissingInputHeadCommitOid => {
            UseCaseInfraError::workspace_invalid_state(error).into()
        }
        error @ (WorkspaceError::WorkspaceRefMismatch { .. }
        | WorkspaceError::WorkspaceHeadMismatch { .. }) => {
            UseCaseInfraError::workspace_state_mismatch(error).into()
        }
        other => UseCaseInfraError::external("workspace", other).into(),
    }
}

pub(crate) async fn drain_until_idle<F, Fut>(mut step: F) -> Result<(), RuntimeError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<bool, RuntimeError>>,
{
    while step().await? {}
    Ok(())
}
