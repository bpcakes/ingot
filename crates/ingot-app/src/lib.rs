use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ingot_domain::ids::{ItemId, ProjectId, WorkspaceId};
use ingot_domain::ports::RevisionLaneTeardownMutation;
use ingot_domain::project::Project;
use ingot_domain::revision::ItemRevision;
use ingot_git::GitJobCompletionPort;
use ingot_git::project_repo::project_repo_paths_for_project;
use ingot_store_sqlite::Database;
use ingot_usecases::application::refresh_revision_context_for_item;
use ingot_usecases::{
    CompleteJobService, DispatchNotify, ProjectLocks, UiEventBus, UseCaseError,
    application::ApplicationInfraPort, dispatch::DispatchInfraPort, workspace::WorkspaceInfraPort,
};

use crate::errors::repo_to_item_usecase;
pub use convergence::RejectApprovalTeardown;

mod convergence;
mod errors;
mod infra;

pub type ApplicationDatabase = Database;
pub type ApplicationCompleteJobService =
    CompleteJobService<ApplicationDatabase, GitJobCompletionPort, ProjectLocks>;

#[derive(Clone)]
pub struct ApplicationServices {
    db: ApplicationDatabase,
    complete_job_service: ApplicationCompleteJobService,
    infra: infra::ApplicationInfra,
    project_locks: ProjectLocks,
    dispatch_notify: DispatchNotify,
    ui_events: UiEventBus,
    state_root: PathBuf,
}

impl ApplicationServices {
    pub fn new(
        db: ApplicationDatabase,
        project_locks: ProjectLocks,
        state_root: PathBuf,
        dispatch_notify: DispatchNotify,
        ui_events: UiEventBus,
    ) -> Self {
        let repo_path_resolver_root = state_root.clone();
        let complete_job_service = CompleteJobService::with_repo_path_resolver(
            db.clone(),
            GitJobCompletionPort,
            project_locks.clone(),
            Arc::new(move |project: &Project| {
                project_repo_paths_for_project(repo_path_resolver_root.as_path(), project)
                    .mirror_git_dir
            }),
        );
        let infra = infra::ApplicationInfra::new(db.clone(), state_root.clone());
        Self {
            db,
            complete_job_service,
            infra,
            project_locks,
            dispatch_notify,
            ui_events,
            state_root,
        }
    }

    pub fn infra(
        &self,
    ) -> impl ApplicationInfraPort + DispatchInfraPort + WorkspaceInfraPort + Clone + 'static {
        self.runtime_infra().clone()
    }

    pub async fn queue_prepare_convergence(
        &self,
        project_id: ProjectId,
        item_id: ItemId,
    ) -> Result<(), UseCaseError> {
        convergence::queue_prepare_convergence(self, project_id, item_id).await
    }

    pub async fn approve_item(
        &self,
        project_id: ProjectId,
        item_id: ItemId,
    ) -> Result<(), UseCaseError> {
        convergence::approve_item(self, project_id, item_id).await
    }

    pub async fn reject_item_approval(
        &self,
        project_id: ProjectId,
        item_id: ItemId,
        next_revision: &ItemRevision,
    ) -> Result<RejectApprovalTeardown, UseCaseError> {
        convergence::reject_item_approval(self, project_id, item_id, next_revision).await
    }

    pub fn db(&self) -> &ApplicationDatabase {
        &self.db
    }

    pub fn complete_job_service(&self) -> &ApplicationCompleteJobService {
        &self.complete_job_service
    }

    pub fn project_locks(&self) -> &ProjectLocks {
        &self.project_locks
    }

    pub fn dispatch_notify(&self) -> &DispatchNotify {
        &self.dispatch_notify
    }

    pub fn ui_events(&self) -> &UiEventBus {
        &self.ui_events
    }

    pub fn state_root(&self) -> &Path {
        self.state_root.as_path()
    }

    pub fn job_logs_dir(&self, job_id: impl Display) -> PathBuf {
        ingot_config::paths::job_logs_dir(self.state_root.as_path(), job_id)
    }

    fn runtime_infra(&self) -> &infra::ApplicationInfra {
        &self.infra
    }
}

#[derive(Default)]
pub(crate) struct RevisionLaneTeardown {
    cancelled_convergence_ids: Vec<String>,
    cancelled_queue_entry_ids: Vec<String>,
}

pub(crate) struct RevisionLaneTeardownPlan {
    pub(crate) teardown: RevisionLaneTeardown,
    pub(crate) mutation: RevisionLaneTeardownMutation,
    pub(crate) integration_workspace_ids: Vec<WorkspaceId>,
}

impl RevisionLaneTeardown {
    pub fn has_cancelled_convergence(&self) -> bool {
        !self.cancelled_convergence_ids.is_empty()
    }

    pub fn has_cancelled_queue_entry(&self) -> bool {
        !self.cancelled_queue_entry_ids.is_empty()
    }

    pub fn first_cancelled_convergence_id(&self) -> Option<&str> {
        self.cancelled_convergence_ids.first().map(String::as_str)
    }

    pub fn first_cancelled_queue_entry_id(&self) -> Option<&str> {
        self.cancelled_queue_entry_ids.first().map(String::as_str)
    }
}

pub(crate) async fn plan_revision_lane_state(
    services: &ApplicationServices,
    project_id: ProjectId,
    item_id: ItemId,
    revision: &ItemRevision,
) -> Result<RevisionLaneTeardownPlan, UseCaseError> {
    let uc_plan = ingot_usecases::teardown::plan_revision_lane_teardown(
        &services.db,
        project_id,
        item_id,
        revision,
    )
    .await?;

    // The app layer only exposes cancellation summaries used by approval-rejection
    // activity. Lower-level job and git-operation teardown ids stay in the usecase
    // result unless a caller needs them.
    Ok(RevisionLaneTeardownPlan {
        teardown: RevisionLaneTeardown {
            cancelled_convergence_ids: uc_plan
                .result
                .cancelled_convergence_ids
                .iter()
                .map(ToString::to_string)
                .collect(),
            cancelled_queue_entry_ids: uc_plan
                .result
                .cancelled_queue_entry_ids
                .iter()
                .map(ToString::to_string)
                .collect(),
        },
        mutation: uc_plan.mutation,
        integration_workspace_ids: uc_plan.result.integration_workspace_ids,
    })
}

pub(crate) async fn refresh_and_cleanup_revision_lane_state(
    services: &ApplicationServices,
    project: &Project,
    item_id: ItemId,
    revision: &ItemRevision,
    integration_workspace_ids: &[WorkspaceId],
) -> Result<(), UseCaseError> {
    let item = services
        .db
        .get_item(item_id)
        .await
        .map_err(repo_to_item_usecase)?;
    let infra = services.runtime_infra();
    refresh_revision_context_for_item(&services.db, infra, &item, revision).await?;

    for workspace_id in integration_workspace_ids {
        let workspace = services
            .db
            .get_workspace(*workspace_id)
            .await
            .map_err(UseCaseError::Repository)?;
        if workspace.path.exists() {
            let _ = infra
                .remove_workspace_path(project.id, &workspace.path)
                .await;
        }
    }

    Ok(())
}
