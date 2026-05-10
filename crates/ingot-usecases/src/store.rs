use std::future::Future;

use ingot_domain::convergence::Convergence;
use ingot_domain::convergence_queue::ConvergenceQueueEntry;
use ingot_domain::git_operation::GitOperation;
use ingot_domain::ids::{ItemId, ItemRevisionId, WorkspaceId};
use ingot_domain::job::Job;
use ingot_domain::ports::{
    ActivityRepository, ConvergenceQueueRepository, ConvergenceRepository, FinalizationRepository,
    FindingRepository, GitOperationRepository, InvalidatePreparedConvergenceRepository,
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

pub trait DispatchStore:
    JobRepository + WorkspaceRepository + GitOperationRepository + ActivityRepository
{
}

pub trait InvestigationRefStore: GitOperationRepository + ActivityRepository {}

pub trait DispatchCleanupStore: WorkspaceRepository + GitOperationRepository {}

pub trait AutoDispatchStore: JobRepository + WorkspaceRepository + ActivityRepository {}

pub trait FindingCleanupStore:
    FindingRepository + GitOperationRepository + ActivityRepository
{
}

pub trait CreateItemStore: ProjectRepository + ItemRepository + ActivityRepository {}

pub trait UpdateItemStore: ProjectRepository + ItemRepository + ActivityRepository {}

pub trait ItemRevisionMutationStore:
    ProjectRepository
    + ItemRepository
    + RevisionRepository
    + JobRepository
    + WorkspaceRepository
    + ActivityRepository
    + RevisionLaneTeardownSideEffectStore
{
}

pub trait ResumeItemStore:
    ProjectRepository + ItemRepository + ActivityRepository + ProjectedReviewDispatchStore
{
}

pub trait ReopenItemStore:
    ProjectRepository
    + ItemRepository
    + RevisionRepository
    + JobRepository
    + WorkspaceRepository
    + ActivityRepository
{
}

pub trait ProjectedReviewDispatchStore:
    ItemRepository + ItemRuntimeSnapshotStore + AutoDispatchStore
{
}

pub trait ApplyFindingTriageStore:
    ProjectRepository
    + ItemRepository
    + RevisionRepository
    + JobRepository
    + FindingRepository
    + ActivityRepository
    + ProjectedReviewDispatchStore
    + FindingCleanupStore
{
}

pub trait PromoteFindingStore: ApplyFindingTriageStore + DispatchStore {}

pub trait BatchPromoteFindingsStore:
    ProjectRepository
    + FindingRepository
    + ItemRepository
    + RevisionRepository
    + JobRepository
    + ActivityRepository
{
}

pub trait AutoTriageStore:
    FindingRepository + RevisionRepository + ItemRepository + ActivityRepository
{
}

pub trait JobWorkflowStore:
    JobRepository
    + ItemRepository
    + ProjectRepository
    + ActivityRepository
    + ApplicationJobContextStore
    + ItemRuntimeSnapshotStore
    + AutoDispatchStore
{
}

pub trait WorkspaceCommandStore:
    WorkspaceRepository + GitOperationRepository + ActivityRepository
{
}

pub trait ConvergenceQueuePromotionStore: ConvergenceQueueRepository + ActivityRepository {}

pub trait PreparedConvergenceInvalidationStore:
    WorkspaceRepository + InvalidatePreparedConvergenceRepository
{
}

pub trait FinalizeOperationStore:
    GitOperationRepository + ActivityRepository + FinalizationRepository
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

impl<T> DispatchStore for T where
    T: JobRepository + WorkspaceRepository + GitOperationRepository + ActivityRepository
{
}

impl<T> InvestigationRefStore for T where T: GitOperationRepository + ActivityRepository {}

impl<T> DispatchCleanupStore for T where T: WorkspaceRepository + GitOperationRepository {}

impl<T> AutoDispatchStore for T where T: JobRepository + WorkspaceRepository + ActivityRepository {}

impl<T> FindingCleanupStore for T where
    T: FindingRepository + GitOperationRepository + ActivityRepository
{
}

impl<T> CreateItemStore for T where T: ProjectRepository + ItemRepository + ActivityRepository {}

impl<T> UpdateItemStore for T where T: ProjectRepository + ItemRepository + ActivityRepository {}

impl<T> ItemRevisionMutationStore for T where
    T: ProjectRepository
        + ItemRepository
        + RevisionRepository
        + JobRepository
        + WorkspaceRepository
        + ActivityRepository
        + RevisionLaneTeardownSideEffectStore
{
}

impl<T> ResumeItemStore for T where
    T: ProjectRepository + ItemRepository + ActivityRepository + ProjectedReviewDispatchStore
{
}

impl<T> ReopenItemStore for T where
    T: ProjectRepository
        + ItemRepository
        + RevisionRepository
        + JobRepository
        + WorkspaceRepository
        + ActivityRepository
{
}

impl<T> ProjectedReviewDispatchStore for T where
    T: ItemRepository + ItemRuntimeSnapshotStore + AutoDispatchStore
{
}

impl<T> ApplyFindingTriageStore for T where
    T: ProjectRepository
        + ItemRepository
        + RevisionRepository
        + JobRepository
        + FindingRepository
        + ActivityRepository
        + ProjectedReviewDispatchStore
        + FindingCleanupStore
{
}

impl<T> PromoteFindingStore for T where T: ApplyFindingTriageStore + DispatchStore {}

impl<T> BatchPromoteFindingsStore for T where
    T: ProjectRepository
        + FindingRepository
        + ItemRepository
        + RevisionRepository
        + JobRepository
        + ActivityRepository
{
}

impl<T> AutoTriageStore for T where
    T: FindingRepository + RevisionRepository + ItemRepository + ActivityRepository
{
}

impl<T> JobWorkflowStore for T where
    T: JobRepository
        + ItemRepository
        + ProjectRepository
        + ActivityRepository
        + ApplicationJobContextStore
        + ItemRuntimeSnapshotStore
        + AutoDispatchStore
{
}

impl<T> WorkspaceCommandStore for T where
    T: WorkspaceRepository + GitOperationRepository + ActivityRepository
{
}

impl<T> ConvergenceQueuePromotionStore for T where T: ConvergenceQueueRepository + ActivityRepository
{}

impl<T> PreparedConvergenceInvalidationStore for T where
    T: WorkspaceRepository + InvalidatePreparedConvergenceRepository
{
}

impl<T> FinalizeOperationStore for T where
    T: GitOperationRepository + ActivityRepository + FinalizationRepository
{
}
