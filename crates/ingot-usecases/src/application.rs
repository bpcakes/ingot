use std::future::Future;
use std::path::Path;

use chrono::Utc;
use ingot_domain::commit_oid::CommitOid;
use ingot_domain::convergence::{CheckoutAdoptionState, Convergence, ConvergenceStatus};
use ingot_domain::convergence_queue::ConvergenceQueueEntryStatus;
use ingot_domain::finding::Finding;
use ingot_domain::git_ref::GitRef;
use ingot_domain::ids::{ItemId, JobId, ProjectId};
use ingot_domain::item::Item;
use ingot_domain::job::Job;
use ingot_domain::ports::{
    ConvergenceQueueRepository, ConvergenceRepository, FindingRepository, ItemRepository,
    JobRepository, ProjectRepository, RevisionRepository, WorkspaceRepository,
};
use ingot_domain::project::Project;
use ingot_domain::revision::ItemRevision;
use ingot_workflow::{
    AllowedAction, BoardStatus, Evaluation, Evaluator, NamedRecommendedAction, PhaseStatus,
    RecommendedAction,
};

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

#[derive(Clone, Debug)]
pub struct ItemProjection {
    pub evaluation: Evaluation,
    pub finalization: FinalizationStatus,
    pub queue: QueueStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalizationPhase {
    None,
    ReadyToFinalize,
    TargetRefAdvanced,
}

#[derive(Clone, Debug)]
pub struct FinalizationStatus {
    pub phase: FinalizationPhase,
    pub checkout_adoption_state: Option<CheckoutAdoptionState>,
    pub checkout_adoption_message: Option<String>,
    pub final_target_commit_oid: Option<CommitOid>,
}

#[derive(Clone, Debug)]
pub struct QueueStatus {
    pub state: Option<ConvergenceQueueEntryStatus>,
    pub position: Option<u32>,
    pub lane_owner_item_id: Option<ItemId>,
    pub lane_target_ref: Option<GitRef>,
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

pub async fn evaluate_item_snapshot<R>(
    repo: &R,
    project: &Project,
    item: &Item,
    snapshot: &ItemRuntimeSnapshot,
    evaluator: &Evaluator,
) -> Result<ItemProjection, UseCaseError>
where
    R: ConvergenceQueueRepository,
{
    let evaluation = evaluator.evaluate(
        item,
        &snapshot.current_revision,
        &snapshot.jobs,
        &snapshot.findings,
        &snapshot.convergences,
    );
    let finalization = load_finalization_status(&snapshot.current_revision, &snapshot.convergences);
    let queue = load_queue_status(repo, &snapshot.current_revision, project).await?;
    let evaluation = overlay_evaluation_with_queue_state(evaluation, &finalization, &queue);

    Ok(ItemProjection {
        evaluation,
        finalization,
        queue,
    })
}

fn load_finalization_status(
    revision: &ItemRevision,
    convergences: &[Convergence],
) -> FinalizationStatus {
    let mut current_revision_convergences = convergences
        .iter()
        .filter(|convergence| convergence.item_revision_id == revision.id);

    if let Some(convergence) = current_revision_convergences
        .clone()
        .find(|convergence| convergence.state.status() == ConvergenceStatus::Finalized)
    {
        return FinalizationStatus {
            phase: FinalizationPhase::TargetRefAdvanced,
            checkout_adoption_state: convergence.state.checkout_adoption_state(),
            checkout_adoption_message: convergence
                .state
                .checkout_adoption_message()
                .map(ToOwned::to_owned),
            final_target_commit_oid: convergence.state.final_target_commit_oid().cloned(),
        };
    }

    if let Some(convergence) = current_revision_convergences
        .find(|convergence| convergence.state.status() == ConvergenceStatus::Prepared)
    {
        return FinalizationStatus {
            phase: FinalizationPhase::ReadyToFinalize,
            checkout_adoption_state: None,
            checkout_adoption_message: None,
            final_target_commit_oid: convergence.state.prepared_commit_oid().cloned(),
        };
    }

    FinalizationStatus {
        phase: FinalizationPhase::None,
        checkout_adoption_state: None,
        checkout_adoption_message: None,
        final_target_commit_oid: None,
    }
}

fn empty_queue_status() -> QueueStatus {
    QueueStatus {
        state: None,
        position: None,
        lane_owner_item_id: None,
        lane_target_ref: None,
    }
}

async fn load_queue_status<R>(
    repo: &R,
    revision: &ItemRevision,
    project: &Project,
) -> Result<QueueStatus, UseCaseError>
where
    R: ConvergenceQueueRepository,
{
    let Some(active_entry) =
        <R as ConvergenceQueueRepository>::find_active_for_revision(repo, revision.id).await?
    else {
        return Ok(empty_queue_status());
    };

    let lane_entries = <R as ConvergenceQueueRepository>::list_active_for_lane(
        repo,
        project.id,
        &revision.target_ref,
    )
    .await?;
    let lane_owner_item_id = lane_entries
        .iter()
        .find(|entry| entry.status == ConvergenceQueueEntryStatus::Head)
        .map(|entry| entry.item_id);
    let position = lane_entries
        .iter()
        .position(|entry| entry.id == active_entry.id)
        .map(|index| index as u32 + 1);

    Ok(QueueStatus {
        state: Some(active_entry.status),
        position,
        lane_owner_item_id,
        lane_target_ref: Some(active_entry.target_ref),
    })
}

fn overlay_evaluation_with_queue_state(
    mut evaluation: Evaluation,
    finalization: &FinalizationStatus,
    queue: &QueueStatus,
) -> Evaluation {
    let awaiting_lane = (queue.state.is_some()
        && evaluation.next_recommended_action
            == RecommendedAction::named(NamedRecommendedAction::PrepareConvergence))
        || queue.state == Some(ConvergenceQueueEntryStatus::Queued);
    if awaiting_lane {
        set_awaiting_convergence_lane(&mut evaluation);
    }

    let awaiting_checkout_sync = finalization.phase == FinalizationPhase::TargetRefAdvanced
        && matches!(
            finalization.checkout_adoption_state,
            Some(CheckoutAdoptionState::Pending | CheckoutAdoptionState::Blocked)
        );
    if awaiting_checkout_sync {
        evaluation.next_recommended_action =
            RecommendedAction::named(NamedRecommendedAction::ResolveCheckoutSync);
        evaluation.dispatchable_step_id = None;
        evaluation.allowed_actions.clear();
        evaluation.phase_status = Some(PhaseStatus::AwaitingCheckoutSync);
        evaluation.board_status = BoardStatus::Working;
    }

    evaluation
}

fn set_awaiting_convergence_lane(evaluation: &mut Evaluation) {
    evaluation.next_recommended_action =
        RecommendedAction::named(NamedRecommendedAction::AwaitConvergenceLane);
    evaluation.dispatchable_step_id = None;
    evaluation
        .allowed_actions
        .retain(|action| *action != AllowedAction::PrepareConvergence);
    evaluation.phase_status = Some(PhaseStatus::AwaitingConvergence);
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
