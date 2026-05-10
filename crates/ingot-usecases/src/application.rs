use std::future::Future;
use std::path::Path;

use chrono::Utc;
use ingot_domain::commit_oid::CommitOid;
use ingot_domain::convergence::Convergence;
use ingot_domain::finding::Finding;
use ingot_domain::git_ref::GitRef;
use ingot_domain::ids::{ItemId, JobId, ProjectId};
use ingot_domain::item::Item;
use ingot_domain::job::Job;
use ingot_domain::ports::{
    ConvergenceRepository, FindingRepository, ItemRepository, JobRepository, ProjectRepository,
    RevisionRepository, WorkspaceRepository,
};
use ingot_domain::project::Project;
use ingot_domain::revision::ItemRevision;

use crate::UseCaseError;
use crate::dispatch::{
    current_authoring_head_for_revision_with_workspace, effective_authoring_base_commit_oid,
};
use crate::revision_context::rebuild_revision_context;
use crate::store::{
    ApplicationJobContextStore, ItemRuntimeSnapshotStore, RevisionContextStore,
    RevisionLaneTeardownSideEffectStore,
};
use crate::teardown::{RevisionLaneTeardownResult, teardown_revision_lane};

pub trait ApplicationInfraPort: Send + Sync {
    fn ensure_valid_target_ref(
        &self,
        target_ref: &str,
    ) -> impl Future<Output = Result<(), UseCaseError>> + Send;

    fn refresh_project_mirror(
        &self,
        project: &Project,
    ) -> impl Future<Output = Result<(), UseCaseError>> + Send;

    fn resolve_project_ref_oid(
        &self,
        project_id: ProjectId,
        ref_name: &GitRef,
    ) -> impl Future<Output = Result<Option<CommitOid>, UseCaseError>> + Send;

    fn is_commit_reachable_from_any_ref(
        &self,
        project_id: ProjectId,
        commit_oid: &CommitOid,
    ) -> impl Future<Output = Result<bool, UseCaseError>> + Send;

    fn is_commit_reachable_from_project(
        &self,
        project: &Project,
        commit_oid: &CommitOid,
    ) -> impl Future<Output = Result<bool, UseCaseError>> + Send;

    fn changed_paths_between(
        &self,
        project_id: ProjectId,
        base_commit_oid: &CommitOid,
        head_commit_oid: &CommitOid,
    ) -> impl Future<Output = Result<Vec<String>, UseCaseError>> + Send;

    fn remove_workspace_path(
        &self,
        project_id: ProjectId,
        workspace_path: &Path,
    ) -> impl Future<Output = Result<(), UseCaseError>> + Send;
}

#[derive(Clone, Debug)]
pub struct ItemRuntimeSnapshot {
    pub current_revision: ItemRevision,
    pub jobs: Vec<Job>,
    pub findings: Vec<Finding>,
    pub convergences: Vec<Convergence>,
}

pub async fn refresh_revision_context_for_job<R, I>(
    repo: &R,
    infra: &I,
    job_id: JobId,
) -> Result<(), UseCaseError>
where
    R: ApplicationJobContextStore,
    I: ApplicationInfraPort,
{
    let job = <R as JobRepository>::get(repo, job_id).await?;
    let item = <R as ItemRepository>::get(repo, job.item_id).await?;
    let revision = <R as RevisionRepository>::get(repo, job.item_revision_id).await?;
    let project = <R as ProjectRepository>::get(repo, job.project_id).await?;
    infra.refresh_project_mirror(&project).await?;
    refresh_revision_context_for_item(repo, infra, &item, &revision).await
}

pub async fn load_item_runtime_snapshot<R, I>(
    repo: &R,
    infra: &I,
    project_id: ProjectId,
    item: &Item,
) -> Result<ItemRuntimeSnapshot, UseCaseError>
where
    R: ItemRuntimeSnapshotStore,
    I: ApplicationInfraPort,
{
    let current_revision = <R as RevisionRepository>::get(repo, item.current_revision_id).await?;
    let jobs = <R as JobRepository>::list_by_item(repo, item.id).await?;
    let findings = <R as FindingRepository>::list_by_item(repo, item.id).await?;
    let mut convergences = <R as ConvergenceRepository>::list_by_item(repo, item.id).await?;
    hydrate_convergence_validity(infra, project_id, &mut convergences).await?;
    Ok(ItemRuntimeSnapshot {
        current_revision,
        jobs,
        findings,
        convergences,
    })
}

pub async fn hydrate_convergence_validity<I>(
    infra: &I,
    project_id: ProjectId,
    convergences: &mut [Convergence],
) -> Result<(), UseCaseError>
where
    I: ApplicationInfraPort,
{
    for convergence in convergences {
        let resolved = infra
            .resolve_project_ref_oid(project_id, &convergence.target_ref)
            .await?;
        convergence.target_head_valid =
            convergence.target_head_valid_for_resolved_oid(resolved.as_ref());
    }

    Ok(())
}

pub async fn refresh_revision_context_for_item<R, I>(
    repo: &R,
    infra: &I,
    item: &Item,
    revision: &ItemRevision,
) -> Result<(), UseCaseError>
where
    R: RevisionContextStore,
    I: ApplicationInfraPort,
{
    let jobs = <R as JobRepository>::list_by_item(repo, item.id).await?;
    let workspace = repo.find_authoring_for_revision(revision.id).await?;
    let authoring_head_commit_oid =
        current_authoring_head_for_revision_with_workspace(revision, &jobs, workspace.as_ref());
    let authoring_base_commit_oid =
        effective_authoring_base_commit_oid(revision, workspace.as_ref());
    let changed_paths = if let (Some(base_commit_oid), Some(head_commit_oid)) = (
        authoring_base_commit_oid.as_ref(),
        authoring_head_commit_oid.as_ref(),
    ) {
        infra
            .changed_paths_between(item.project_id, base_commit_oid, head_commit_oid)
            .await?
    } else {
        Vec::new()
    };
    let context = rebuild_revision_context(
        item,
        revision,
        &jobs,
        authoring_head_commit_oid,
        changed_paths,
        jobs.first().map(|job| job.id),
        Utc::now(),
    );
    repo.upsert(&context).await?;
    Ok(())
}

pub async fn teardown_revision_lane_with_side_effects<R, I>(
    repo: &R,
    infra: &I,
    project: &Project,
    item_id: ItemId,
    revision: &ItemRevision,
) -> Result<RevisionLaneTeardownResult, UseCaseError>
where
    R: RevisionLaneTeardownSideEffectStore,
    I: ApplicationInfraPort,
{
    let result = teardown_revision_lane(repo, project.id, item_id, revision).await?;

    let item = <R as ItemRepository>::get(repo, item_id).await?;
    refresh_revision_context_for_item(repo, infra, &item, revision).await?;

    for workspace_id in &result.integration_workspace_ids {
        let workspace = <R as WorkspaceRepository>::get(repo, *workspace_id).await?;
        if workspace.path.exists() {
            let _ = infra
                .remove_workspace_path(project.id, &workspace.path)
                .await;
        }
    }

    Ok(result)
}
