use chrono::Utc;
use ingot_domain::activity::{Activity, ActivityEventType, ActivitySubject};
use ingot_domain::commit_oid::CommitOid;
use ingot_domain::git_ref::GitRef;
use ingot_domain::ids::{ActivityId, ItemId, ProjectId};
use ingot_domain::item::{
    ApprovalState, Classification, DoneReason, Escalation, Item, Lifecycle, ParkingState, Priority,
    ResolutionSource,
};
use ingot_domain::job::Job;
use ingot_domain::ports::{
    ActivityRepository, ItemRepository, JobRepository, ProjectMutationLockPort, ProjectRepository,
    RepositoryError, RevisionRepository, WorkspaceRepository,
};
use ingot_domain::project::Project;
use ingot_domain::revision::{ApprovalPolicy, AuthoringBaseSeed, ItemRevision};

use crate::UseCaseError;
use crate::application::{
    ApplicationInfraPort, load_item_runtime_snapshot, teardown_revision_lane_with_side_effects,
};
use crate::dispatch::{auto_dispatch_review, current_authoring_head_for_revision_with_workspace};
use crate::item::{
    CreateInvestigationInput, CreateItemInput, approval_state_for_policy,
    create_investigation_item, create_manual_item, default_policy_snapshot,
    default_template_map_snapshot, next_sort_key, rework_budgets_from_policy_snapshot,
};
use crate::store::{
    CreateItemStore, ItemRevisionMutationStore, ProjectedReviewDispatchStore, ReopenItemStore,
    ResumeItemStore, UpdateItemStore,
};

#[derive(Clone, Debug)]
pub struct CreateItemCommand {
    pub project_id: ProjectId,
    pub title: String,
    pub description: String,
    pub acceptance_criteria: String,
    pub classification: Option<Classification>,
    pub priority: Option<Priority>,
    pub labels: Option<Vec<String>>,
    pub operator_notes: Option<String>,
    pub target_ref: Option<GitRef>,
    pub approval_policy: Option<ApprovalPolicy>,
    pub seed_commit_oid: Option<CommitOid>,
    pub seed_target_commit_oid: Option<CommitOid>,
    pub default_approval_policy: ApprovalPolicy,
    pub candidate_rework_budget: u32,
    pub integration_rework_budget: u32,
}

#[derive(Clone, Debug)]
pub struct UpdateItemCommand {
    pub project_id: ProjectId,
    pub item_id: ItemId,
    pub classification: Option<Classification>,
    pub priority: Option<Priority>,
    pub labels: Option<Vec<String>>,
    pub operator_notes: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ReviseItemCommand {
    pub title: Option<String>,
    pub description: Option<String>,
    pub acceptance_criteria: Option<String>,
    pub target_ref: Option<GitRef>,
    pub approval_policy: Option<ApprovalPolicy>,
    pub seed_commit_oid: Option<CommitOid>,
    pub seed_target_commit_oid: Option<CommitOid>,
}

#[derive(Clone, Copy, Debug)]
pub struct ItemCommandOutput {
    pub item_id: ItemId,
}

pub type AutoDispatchResult = Result<Option<Job>, UseCaseError>;

pub async fn create_item<R, I, L>(
    repo: &R,
    infra: &I,
    project_locks: &L,
    command: CreateItemCommand,
) -> Result<ItemCommandOutput, UseCaseError>
where
    R: CreateItemStore,
    I: ApplicationInfraPort,
    L: ProjectMutationLockPort,
{
    let project = <R as ProjectRepository>::get(repo, command.project_id)
        .await
        .map_err(map_project_get_error)?;
    let _guard = project_locks
        .acquire_project_mutation(command.project_id)
        .await;
    let target_ref = GitRef::parse_target_ref(
        command
            .target_ref
            .as_ref()
            .map(GitRef::as_str)
            .unwrap_or(project.default_branch.as_str()),
    )?;
    infra.ensure_valid_target_ref(target_ref.as_str()).await?;
    let resolved_target_head = infra
        .resolve_project_ref_oid(project.id, &target_ref)
        .await?
        .ok_or_else(|| UseCaseError::TargetRefUnresolved(target_ref.to_string()))?;

    let sort_key = next_project_sort_key(repo, project.id).await?;
    let classification = command.classification.unwrap_or(Classification::Change);
    let labels = command.labels.unwrap_or_default();

    let (item, revision) = if classification == Classification::Investigation {
        create_investigation_item(
            &project,
            CreateInvestigationInput {
                title: command.title,
                description: command.description,
                target_ref,
                priority: command.priority.unwrap_or(Priority::Major),
                labels,
                operator_notes: command.operator_notes,
                target_ref_head: resolved_target_head,
            },
            sort_key,
            Utc::now(),
        )
    } else {
        let seed_commit_oid =
            validate_seed_commit_oid(infra, project.id, command.seed_commit_oid).await?;
        let seed_target_commit_oid = resolve_seed_target_commit_oid(
            infra,
            project.id,
            command.seed_target_commit_oid,
            resolved_target_head,
        )
        .await?;
        let seed = AuthoringBaseSeed::from_parts(seed_commit_oid, seed_target_commit_oid);

        create_manual_item(
            &project,
            CreateItemInput {
                classification,
                priority: command.priority.unwrap_or(Priority::Major),
                labels,
                operator_notes: command.operator_notes,
                title: command.title,
                description: command.description,
                acceptance_criteria: command.acceptance_criteria,
                target_ref,
                approval_policy: command
                    .approval_policy
                    .unwrap_or(command.default_approval_policy),
                candidate_rework_budget: command.candidate_rework_budget,
                integration_rework_budget: command.integration_rework_budget,
                seed,
            },
            sort_key,
            Utc::now(),
        )
    };

    <R as ItemRepository>::create_with_revision(repo, &item, &revision).await?;
    append_activity(
        repo,
        project.id,
        ActivityEventType::ItemCreated,
        ActivitySubject::Item(item.id),
        serde_json::json!({ "revision_id": revision.id }),
    )
    .await?;

    Ok(ItemCommandOutput { item_id: item.id })
}

pub async fn update_item<R, L>(
    repo: &R,
    project_locks: &L,
    command: UpdateItemCommand,
) -> Result<ItemCommandOutput, UseCaseError>
where
    R: UpdateItemStore,
    L: ProjectMutationLockPort,
{
    let _project = <R as ProjectRepository>::get(repo, command.project_id)
        .await
        .map_err(map_project_get_error)?;
    let _guard = project_locks
        .acquire_project_mutation(command.project_id)
        .await;
    let mut item = <R as ItemRepository>::get(repo, command.item_id)
        .await
        .map_err(map_item_get_error)?;
    ensure_item_project(&item, command.project_id)?;
    if let Some(classification) = command.classification {
        item.classification = classification;
    }
    if let Some(priority) = command.priority {
        item.priority = priority;
    }
    if let Some(labels) = command.labels {
        item.labels = labels;
    }
    if command.operator_notes.is_some() {
        item.operator_notes = command.operator_notes;
    }
    item.updated_at = Utc::now();
    <R as ItemRepository>::update(repo, &item).await?;
    append_activity(
        repo,
        command.project_id,
        ActivityEventType::ItemUpdated,
        ActivitySubject::Item(item.id),
        serde_json::json!({}),
    )
    .await?;
    Ok(ItemCommandOutput { item_id: item.id })
}

pub async fn revise_item<R, I, L>(
    repo: &R,
    infra: &I,
    project_locks: &L,
    project_id: ProjectId,
    item_id: ItemId,
    command: ReviseItemCommand,
) -> Result<ItemCommandOutput, UseCaseError>
where
    R: ItemRevisionMutationStore,
    I: ApplicationInfraPort,
    L: ProjectMutationLockPort,
{
    let project = <R as ProjectRepository>::get(repo, project_id)
        .await
        .map_err(map_project_get_error)?;
    let _guard = project_locks.acquire_project_mutation(project_id).await;
    let mut item = <R as ItemRepository>::get(repo, item_id)
        .await
        .map_err(map_item_get_error)?;
    ensure_item_project(&item, project_id)?;
    ensure_item_open_idle(&item)?;
    let current_revision = <R as RevisionRepository>::get(repo, item.current_revision_id).await?;
    let _ =
        teardown_revision_lane_with_side_effects(repo, infra, &project, item.id, &current_revision)
            .await?;
    let jobs = <R as JobRepository>::list_by_item(repo, item.id).await?;
    let next_revision = build_superseding_revision(
        repo,
        infra,
        &project,
        &item,
        &current_revision,
        &jobs,
        command,
    )
    .await?;
    <R as RevisionRepository>::create(repo, &next_revision).await?;
    item.current_revision_id = next_revision.id;
    let cleared_escalation = item.escalation.is_escalated();
    item.approval_state = approval_state_for_policy(next_revision.approval_policy);
    item.escalation = Escalation::None;
    item.updated_at = Utc::now();
    <R as ItemRepository>::update(repo, &item).await?;
    append_activity(
        repo,
        project_id,
        ActivityEventType::ItemRevisionCreated,
        ActivitySubject::Item(item.id),
        serde_json::json!({ "revision_id": next_revision.id, "kind": "revise" }),
    )
    .await?;
    if cleared_escalation {
        append_activity(
            repo,
            project_id,
            ActivityEventType::ItemEscalationCleared,
            ActivitySubject::Item(item.id),
            serde_json::json!({ "reason": "revise" }),
        )
        .await?;
    }
    Ok(ItemCommandOutput { item_id: item.id })
}

pub async fn defer_item<R, I, L>(
    repo: &R,
    infra: &I,
    project_locks: &L,
    project_id: ProjectId,
    item_id: ItemId,
) -> Result<ItemCommandOutput, UseCaseError>
where
    R: ItemRevisionMutationStore,
    I: ApplicationInfraPort,
    L: ProjectMutationLockPort,
{
    let project = <R as ProjectRepository>::get(repo, project_id)
        .await
        .map_err(map_project_get_error)?;
    let _guard = project_locks.acquire_project_mutation(project_id).await;
    let mut item = <R as ItemRepository>::get(repo, item_id)
        .await
        .map_err(map_item_get_error)?;
    ensure_item_project(&item, project_id)?;
    ensure_item_open_idle(&item)?;
    if item.approval_state == ApprovalState::Pending {
        return Err(UseCaseError::PendingApprovalCannotDefer);
    }
    let current_revision = <R as RevisionRepository>::get(repo, item.current_revision_id).await?;
    let _ =
        teardown_revision_lane_with_side_effects(repo, infra, &project, item.id, &current_revision)
            .await?;
    item.parking_state = ParkingState::Deferred;
    item.approval_state = approval_state_for_policy(current_revision.approval_policy);
    item.escalation = Escalation::None;
    item.updated_at = Utc::now();
    <R as ItemRepository>::update(repo, &item).await?;
    append_activity(
        repo,
        project_id,
        ActivityEventType::ItemDeferred,
        ActivitySubject::Item(item.id),
        serde_json::json!({}),
    )
    .await?;
    Ok(ItemCommandOutput { item_id: item.id })
}

pub async fn resume_item<R, I, L>(
    repo: &R,
    infra: &I,
    project_locks: &L,
    project_id: ProjectId,
    item_id: ItemId,
) -> Result<(ItemCommandOutput, AutoDispatchResult), UseCaseError>
where
    R: ResumeItemStore,
    I: ApplicationInfraPort,
    L: ProjectMutationLockPort,
{
    let project = <R as ProjectRepository>::get(repo, project_id)
        .await
        .map_err(map_project_get_error)?;
    let _guard = project_locks.acquire_project_mutation(project_id).await;
    let mut item = <R as ItemRepository>::get(repo, item_id)
        .await
        .map_err(map_item_get_error)?;
    ensure_item_project(&item, project_id)?;
    if item.parking_state != ParkingState::Deferred {
        return Err(UseCaseError::ItemNotDeferred);
    }
    item.parking_state = ParkingState::Active;
    item.updated_at = Utc::now();
    <R as ItemRepository>::update(repo, &item).await?;
    append_activity(
        repo,
        project_id,
        ActivityEventType::ItemResumed,
        ActivitySubject::Item(item.id),
        serde_json::json!({}),
    )
    .await?;
    let dispatch_result = auto_dispatch_projected_review_job(repo, infra, &project, item.id).await;
    Ok((ItemCommandOutput { item_id: item.id }, dispatch_result))
}

pub async fn finish_item_manually<R, I, L>(
    repo: &R,
    infra: &I,
    project_locks: &L,
    project_id: ProjectId,
    item_id: ItemId,
    done_reason: DoneReason,
    event_type: ActivityEventType,
) -> Result<ItemCommandOutput, UseCaseError>
where
    R: ItemRevisionMutationStore,
    I: ApplicationInfraPort,
    L: ProjectMutationLockPort,
{
    let project = <R as ProjectRepository>::get(repo, project_id)
        .await
        .map_err(map_project_get_error)?;
    let _guard = project_locks.acquire_project_mutation(project_id).await;
    let mut item = <R as ItemRepository>::get(repo, item_id)
        .await
        .map_err(map_item_get_error)?;
    ensure_item_project(&item, project_id)?;
    ensure_item_open_idle(&item)?;
    let revision = <R as RevisionRepository>::get(repo, item.current_revision_id).await?;
    let _ =
        teardown_revision_lane_with_side_effects(repo, infra, &project, item.id, &revision).await?;
    item.lifecycle = Lifecycle::Done {
        reason: done_reason,
        source: ResolutionSource::ManualCommand,
        closed_at: Utc::now(),
    };
    item.approval_state = approval_state_for_policy(revision.approval_policy);
    item.escalation = Escalation::None;
    item.updated_at = Utc::now();
    <R as ItemRepository>::update(repo, &item).await?;
    append_activity(
        repo,
        project_id,
        event_type,
        ActivitySubject::Item(item.id),
        serde_json::json!({ "done_reason": item.lifecycle.done_reason() }),
    )
    .await?;
    Ok(ItemCommandOutput { item_id: item.id })
}

pub async fn reopen_item<R, I, L>(
    repo: &R,
    infra: &I,
    project_locks: &L,
    project_id: ProjectId,
    item_id: ItemId,
    command: ReviseItemCommand,
) -> Result<ItemCommandOutput, UseCaseError>
where
    R: ReopenItemStore,
    I: ApplicationInfraPort,
    L: ProjectMutationLockPort,
{
    let project = <R as ProjectRepository>::get(repo, project_id)
        .await
        .map_err(map_project_get_error)?;
    let _guard = project_locks.acquire_project_mutation(project_id).await;
    let mut item = <R as ItemRepository>::get(repo, item_id)
        .await
        .map_err(map_item_get_error)?;
    ensure_item_project(&item, project_id)?;
    match item.lifecycle {
        Lifecycle::Done {
            reason: DoneReason::Dismissed | DoneReason::Invalidated,
            ..
        } => {}
        Lifecycle::Done {
            reason: DoneReason::Completed,
            ..
        } => return Err(UseCaseError::CompletedItemCannotReopen),
        Lifecycle::Open => {
            return Err(UseCaseError::ItemNotReopenable);
        }
    }
    let current_revision = <R as RevisionRepository>::get(repo, item.current_revision_id).await?;
    let jobs = <R as JobRepository>::list_by_item(repo, item.id).await?;
    let next_revision = build_superseding_revision(
        repo,
        infra,
        &project,
        &item,
        &current_revision,
        &jobs,
        command,
    )
    .await?;
    <R as RevisionRepository>::create(repo, &next_revision).await?;
    let cleared_escalation = item.escalation.is_escalated();
    item.current_revision_id = next_revision.id;
    item.lifecycle = Lifecycle::Open;
    item.parking_state = ParkingState::Active;
    item.approval_state = approval_state_for_policy(next_revision.approval_policy);
    item.escalation = Escalation::None;
    item.updated_at = Utc::now();
    <R as ItemRepository>::update(repo, &item).await?;
    append_activity(
        repo,
        project_id,
        ActivityEventType::ItemReopened,
        ActivitySubject::Item(item.id),
        serde_json::json!({ "revision_id": next_revision.id }),
    )
    .await?;
    if cleared_escalation {
        append_activity(
            repo,
            project_id,
            ActivityEventType::ItemEscalationCleared,
            ActivitySubject::Item(item.id),
            serde_json::json!({ "reason": "reopen" }),
        )
        .await?;
    }
    Ok(ItemCommandOutput { item_id: item.id })
}

pub async fn auto_dispatch_projected_review_job<R, I>(
    repo: &R,
    infra: &I,
    project: &Project,
    item_id: ItemId,
) -> AutoDispatchResult
where
    R: ProjectedReviewDispatchStore,
    I: ApplicationInfraPort,
{
    let item = <R as ItemRepository>::get(repo, item_id)
        .await
        .map_err(map_item_get_error)?;
    let snapshot = load_item_runtime_snapshot(repo, infra, project.id, &item).await?;
    auto_dispatch_review(
        repo,
        project,
        &item,
        &snapshot.current_revision,
        &snapshot.jobs,
        &snapshot.findings,
        &snapshot.convergences,
    )
    .await
}

fn ensure_item_project(item: &Item, project_id: ProjectId) -> Result<(), UseCaseError> {
    if item.project_id != project_id {
        return Err(UseCaseError::ItemNotFound);
    }
    Ok(())
}

fn map_project_get_error(error: RepositoryError) -> UseCaseError {
    match error {
        RepositoryError::NotFound => UseCaseError::ProjectNotFound,
        other => UseCaseError::Repository(other),
    }
}

fn map_item_get_error(error: RepositoryError) -> UseCaseError {
    match error {
        RepositoryError::NotFound => UseCaseError::ItemNotFound,
        other => UseCaseError::Repository(other),
    }
}

pub fn ensure_item_open_idle(item: &Item) -> Result<(), UseCaseError> {
    if !item.lifecycle.is_open() {
        return Err(UseCaseError::ItemNotOpen);
    }
    if item.parking_state != ParkingState::Active {
        return Err(UseCaseError::ItemNotIdle);
    }
    Ok(())
}

async fn build_superseding_revision<R, I>(
    repo: &R,
    infra: &I,
    project: &Project,
    item: &Item,
    current_revision: &ItemRevision,
    jobs: &[Job],
    command: ReviseItemCommand,
) -> Result<ItemRevision, UseCaseError>
where
    R: WorkspaceRepository,
    I: ApplicationInfraPort,
{
    let target_ref = GitRef::parse_target_ref(
        command
            .target_ref
            .as_ref()
            .map(GitRef::as_str)
            .unwrap_or(current_revision.target_ref.as_str()),
    )?;
    infra.ensure_valid_target_ref(target_ref.as_str()).await?;
    let derived_target_head = infra
        .resolve_project_ref_oid(project.id, &target_ref)
        .await?
        .ok_or_else(|| UseCaseError::TargetRefUnresolved(target_ref.to_string()))?;

    let requested_seed_commit_oid =
        validate_seed_commit_oid(infra, project.id, command.seed_commit_oid).await?;
    let seed_commit_oid = match requested_seed_commit_oid {
        Some(seed_commit_oid) => Some(seed_commit_oid),
        None => {
            let workspace = repo
                .find_authoring_for_revision(current_revision.id)
                .await?;
            current_authoring_head_for_revision_with_workspace(
                current_revision,
                jobs,
                workspace.as_ref(),
            )
            .or_else(|| current_revision.seed.seed_commit_oid().cloned())
        }
    };
    let seed_target_commit_oid = resolve_seed_target_commit_oid(
        infra,
        project.id,
        command.seed_target_commit_oid,
        derived_target_head,
    )
    .await?;
    let seed = AuthoringBaseSeed::from_parts(seed_commit_oid, seed_target_commit_oid);
    let approval_policy = command
        .approval_policy
        .unwrap_or(current_revision.approval_policy);
    let policy_snapshot = build_superseding_policy_snapshot(current_revision, approval_policy);

    Ok(ItemRevision {
        id: ingot_domain::ids::ItemRevisionId::new(),
        item_id: item.id,
        revision_no: current_revision.revision_no + 1,
        title: command.title.unwrap_or(current_revision.title.clone()),
        description: command
            .description
            .unwrap_or(current_revision.description.clone()),
        acceptance_criteria: command
            .acceptance_criteria
            .unwrap_or(current_revision.acceptance_criteria.clone()),
        target_ref,
        approval_policy,
        policy_snapshot,
        template_map_snapshot: default_template_map_snapshot(),
        seed,
        supersedes_revision_id: Some(current_revision.id),
        created_at: Utc::now(),
    })
}

fn build_superseding_policy_snapshot(
    current_revision: &ItemRevision,
    approval_policy: ApprovalPolicy,
) -> serde_json::Value {
    match rework_budgets_from_policy_snapshot(&current_revision.policy_snapshot) {
        Some((candidate_rework_budget, integration_rework_budget)) => default_policy_snapshot(
            approval_policy,
            candidate_rework_budget,
            integration_rework_budget,
        ),
        None => {
            let mut policy_snapshot = current_revision.policy_snapshot.clone();
            if let Some(object) = policy_snapshot.as_object_mut() {
                object.insert(
                    "approval_policy".into(),
                    serde_json::to_value(approval_policy)
                        .expect("approval policy should serialize into JSON"),
                );
            }
            policy_snapshot
        }
    }
}

async fn validate_seed_commit_oid<I>(
    infra: &I,
    project_id: ProjectId,
    seed_commit_oid: Option<CommitOid>,
) -> Result<Option<CommitOid>, UseCaseError>
where
    I: ApplicationInfraPort,
{
    match seed_commit_oid {
        Some(seed_commit_oid) => {
            ensure_reachable_seed(infra, project_id, "seed_commit_oid", &seed_commit_oid).await?;
            Ok(Some(seed_commit_oid))
        }
        None => Ok(None),
    }
}

async fn resolve_seed_target_commit_oid<I>(
    infra: &I,
    project_id: ProjectId,
    seed_target_commit_oid: Option<CommitOid>,
    default_seed_target_commit_oid: CommitOid,
) -> Result<CommitOid, UseCaseError>
where
    I: ApplicationInfraPort,
{
    match seed_target_commit_oid {
        Some(seed_target_commit_oid) => {
            ensure_reachable_seed(
                infra,
                project_id,
                "seed_target_commit_oid",
                &seed_target_commit_oid,
            )
            .await?;
            Ok(seed_target_commit_oid)
        }
        None => Ok(default_seed_target_commit_oid),
    }
}

async fn ensure_reachable_seed<I>(
    infra: &I,
    project_id: ProjectId,
    seed_name: &str,
    commit_oid: &CommitOid,
) -> Result<(), UseCaseError>
where
    I: ApplicationInfraPort,
{
    let reachable = infra
        .is_commit_reachable_from_any_ref(project_id, commit_oid)
        .await?;

    if !reachable {
        return Err(UseCaseError::RevisionSeedUnreachable(seed_name.into()));
    }

    Ok(())
}

async fn next_project_sort_key<R>(repo: &R, project_id: ProjectId) -> Result<String, UseCaseError>
where
    R: ItemRepository,
{
    let items = <R as ItemRepository>::list_by_project(repo, project_id).await?;
    Ok(next_sort_key(&items))
}

async fn append_activity<S>(
    activity_repo: &S,
    project_id: ProjectId,
    event_type: ActivityEventType,
    subject: ActivitySubject,
    payload: serde_json::Value,
) -> Result<(), UseCaseError>
where
    S: ActivityRepository,
{
    <S as ActivityRepository>::append(
        activity_repo,
        &Activity {
            id: ActivityId::new(),
            project_id,
            event_type,
            subject,
            payload,
            created_at: Utc::now(),
        },
    )
    .await?;
    Ok(())
}
