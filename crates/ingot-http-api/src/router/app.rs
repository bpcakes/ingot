use std::path::PathBuf;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::Method;
use axum::middleware;
use axum::response::Response;
use ingot_app::{
    ApplicationCompleteJobService, ApplicationDatabase, ApplicationServices, RejectApprovalTeardown,
};
use ingot_domain::ids::{ItemId, ProjectId};
use ingot_domain::revision::ItemRevision;
use ingot_usecases::{
    DispatchNotify, ProjectLocks, UiEventBus, UseCaseError, application::ApplicationInfraPort,
    dispatch::DispatchInfraPort, workspace::WorkspaceInfraPort,
};

use super::jobs;
use super::{agents, convergence, core, dispatch, findings, harness, items, projects, workspaces};

#[derive(Clone)]
pub(crate) struct AppState {
    services: ApplicationServices,
}

impl AppState {
    pub(crate) fn from_services(services: ApplicationServices) -> Self {
        Self { services }
    }

    #[cfg(test)]
    pub(crate) fn new(
        db: ApplicationDatabase,
        project_locks: ProjectLocks,
        state_root: PathBuf,
        dispatch_notify: DispatchNotify,
        ui_events: UiEventBus,
    ) -> Self {
        let services =
            ApplicationServices::new(db, project_locks, state_root, dispatch_notify, ui_events);
        Self::from_services(services)
    }

    pub(crate) fn infra(
        &self,
    ) -> impl ApplicationInfraPort + DispatchInfraPort + WorkspaceInfraPort + Clone + 'static {
        self.services.infra()
    }

    pub(crate) fn job_logs_dir(&self, job_id: impl std::fmt::Display) -> PathBuf {
        self.services.job_logs_dir(job_id)
    }

    pub(crate) fn db(&self) -> &ApplicationDatabase {
        self.services.db()
    }

    pub(crate) fn complete_job_service(&self) -> &ApplicationCompleteJobService {
        self.services.complete_job_service()
    }

    pub(crate) fn project_locks(&self) -> &ProjectLocks {
        self.services.project_locks()
    }

    pub(crate) fn dispatch_notify(&self) -> &DispatchNotify {
        self.services.dispatch_notify()
    }

    pub(crate) fn ui_events(&self) -> &UiEventBus {
        self.services.ui_events()
    }

    #[cfg(test)]
    pub(crate) fn state_root(&self) -> &std::path::Path {
        self.services.state_root()
    }

    pub(crate) async fn queue_prepare_convergence(
        &self,
        project_id: ProjectId,
        item_id: ItemId,
    ) -> Result<(), UseCaseError> {
        self.services
            .queue_prepare_convergence(project_id, item_id)
            .await
    }

    pub(crate) async fn approve_item(
        &self,
        project_id: ProjectId,
        item_id: ItemId,
    ) -> Result<(), UseCaseError> {
        self.services.approve_item(project_id, item_id).await
    }

    pub(crate) async fn reject_item_approval(
        &self,
        project_id: ProjectId,
        item_id: ItemId,
        next_revision: &ItemRevision,
    ) -> Result<RejectApprovalTeardown, UseCaseError> {
        self.services
            .reject_item_approval(project_id, item_id, next_revision)
            .await
    }
}

/// Build the Axum router with all API routes.
pub fn build_router_with_services(services: ApplicationServices) -> Router {
    let state = AppState::from_services(services);

    Router::new()
        .merge(core::routes())
        .merge(projects::routes())
        .merge(harness::routes())
        .merge(workspaces::routes())
        .merge(agents::routes())
        .merge(items::routes())
        .merge(dispatch::routes())
        .merge(jobs::routes())
        .merge(findings::routes())
        .merge(convergence::routes())
        .merge(super::ws::routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            dispatch_notify_layer,
        ))
        .with_state(state)
}

/// Wakes the background dispatcher after every successful write request.
///
/// Applied to routes that create dispatchable work. Write methods (POST, PUT,
/// PATCH, DELETE) that return 2xx trigger `dispatch_notify.notify()`.
/// Over-notification is harmless because the dispatcher drains until idle.
async fn dispatch_notify_layer(
    State(state): State<AppState>,
    request: Request,
    next: middleware::Next,
) -> Response {
    let should_notify = is_dispatch_write(request.method());
    let notify_reason =
        should_notify.then(|| format!("http {} {}", request.method(), request.uri().path()));
    let response = next.run(request).await;
    if should_notify && response.status().is_success() {
        state.dispatch_notify().notify_with_reason(
            notify_reason.expect("write requests should always have a notify reason"),
        );
    }
    response
}

fn is_dispatch_write(method: &Method) -> bool {
    matches!(
        method,
        &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE
    )
}
