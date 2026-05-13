use std::future::Future;

use ingot_domain::convergence::Convergence;
use ingot_domain::convergence_queue::ConvergenceQueueEntry;
use ingot_domain::git_operation::GitOperation;
use ingot_domain::ids::{ItemId, ItemRevisionId, WorkspaceId};
use ingot_domain::job::Job;
use ingot_domain::ports::{
    ConvergenceQueueRepository, ConvergenceRepository, FindingRepository, GitOperationRepository,
    ItemRepository, JobRepository, ProjectRepository, RepositoryError, RevisionContextRepository,
    RevisionLaneTeardownMutation, RevisionLaneTeardownRepository, RevisionRepository,
    WorkspaceRepository,
};
use ingot_domain::workspace::Workspace;

pub trait ApplicationJobContextStore:
    JobRepository + ItemRepository + RevisionRepository + ProjectRepository + RevisionContextStore
{
}

pub trait ItemRuntimeSnapshotStore:
    RevisionRepository + JobRepository + FindingRepository + ConvergenceRepository
{
}

pub trait RevisionContextStore:
    JobRepository + WorkspaceRepository + RevisionContextRepository
{
}

pub trait RevisionLaneTeardownStore: Send + Sync {
    fn list_teardown_jobs_by_item(
        &self,
        item_id: ItemId,
    ) -> impl Future<Output = Result<Vec<Job>, RepositoryError>> + Send;

    fn list_teardown_convergences_by_item(
        &self,
        item_id: ItemId,
    ) -> impl Future<Output = Result<Vec<Convergence>, RepositoryError>> + Send;

    fn find_active_teardown_queue_entry(
        &self,
        revision_id: ItemRevisionId,
    ) -> impl Future<Output = Result<Option<ConvergenceQueueEntry>, RepositoryError>> + Send;

    fn get_teardown_workspace(
        &self,
        workspace_id: WorkspaceId,
    ) -> impl Future<Output = Result<Workspace, RepositoryError>> + Send;

    fn list_unresolved_teardown_git_operations(
        &self,
    ) -> impl Future<Output = Result<Vec<GitOperation>, RepositoryError>> + Send;

    fn apply_revision_lane_teardown_mutation(
        &self,
        mutation: RevisionLaneTeardownMutation,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send;
}

pub trait RevisionLaneTeardownSideEffectStore:
    RevisionLaneTeardownStore + ItemRepository + RevisionContextStore
{
}

impl<T> ApplicationJobContextStore for T where
    T: JobRepository
        + ItemRepository
        + RevisionRepository
        + ProjectRepository
        + RevisionContextStore
{
}

impl<T> ItemRuntimeSnapshotStore for T where
    T: RevisionRepository + JobRepository + FindingRepository + ConvergenceRepository
{
}

impl<T> RevisionContextStore for T where
    T: JobRepository + WorkspaceRepository + RevisionContextRepository
{
}

impl<T> RevisionLaneTeardownStore for T
where
    T: JobRepository
        + ConvergenceRepository
        + ConvergenceQueueRepository
        + WorkspaceRepository
        + GitOperationRepository
        + RevisionLaneTeardownRepository,
{
    fn list_teardown_jobs_by_item(
        &self,
        item_id: ItemId,
    ) -> impl Future<Output = Result<Vec<Job>, RepositoryError>> + Send {
        <T as JobRepository>::list_by_item(self, item_id)
    }

    fn list_teardown_convergences_by_item(
        &self,
        item_id: ItemId,
    ) -> impl Future<Output = Result<Vec<Convergence>, RepositoryError>> + Send {
        <T as ConvergenceRepository>::list_by_item(self, item_id)
    }

    fn find_active_teardown_queue_entry(
        &self,
        revision_id: ItemRevisionId,
    ) -> impl Future<Output = Result<Option<ConvergenceQueueEntry>, RepositoryError>> + Send {
        <T as ConvergenceQueueRepository>::find_active_for_revision(self, revision_id)
    }

    fn get_teardown_workspace(
        &self,
        workspace_id: WorkspaceId,
    ) -> impl Future<Output = Result<Workspace, RepositoryError>> + Send {
        <T as WorkspaceRepository>::get(self, workspace_id)
    }

    fn list_unresolved_teardown_git_operations(
        &self,
    ) -> impl Future<Output = Result<Vec<GitOperation>, RepositoryError>> + Send {
        <T as GitOperationRepository>::find_unresolved(self)
    }

    fn apply_revision_lane_teardown_mutation(
        &self,
        mutation: RevisionLaneTeardownMutation,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send {
        <T as RevisionLaneTeardownRepository>::apply_revision_lane_teardown(self, mutation)
    }
}

impl<T> RevisionLaneTeardownSideEffectStore for T where
    T: RevisionLaneTeardownStore + ItemRepository + RevisionContextStore
{
}
