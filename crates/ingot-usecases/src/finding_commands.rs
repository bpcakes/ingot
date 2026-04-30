use chrono::Utc;
use ingot_domain::activity::{Activity, ActivityEventType, ActivitySubject};
use ingot_domain::finding::{Finding, FindingSubjectKind, FindingTriageState};
use ingot_domain::git_ref::GitRef;
use ingot_domain::ids::{ActivityId, FindingId, ItemId, ProjectId};
use ingot_domain::item::{ApprovalState, Item};
use ingot_domain::job::Job;
use ingot_domain::ports::{
    ActivityRepository, ConvergenceRepository, FindingRepository, GitOperationRepository,
    ItemRepository, JobRepository, ProjectMutationLockPort, ProjectRepository, RepositoryError,
    RevisionRepository, WorkspaceRepository,
};
use ingot_domain::project::Project;
use ingot_domain::revision::{ApprovalPolicy, ItemRevision};
use ingot_domain::step_id::StepId;
use ingot_workflow::{Evaluator, step};

use crate::UseCaseError;
use crate::application::{ApplicationInfraPort, load_item_runtime_snapshot};
use crate::dispatch::{
    DispatchActivityContext, DispatchInfraPort, maybe_cleanup_investigation_ref,
    prepare_and_persist_dispatched_job,
};
use crate::finding::{
    BacklogFindingOverrides, BatchPromoteInput, BatchPromoteOutput, TriageFindingInput,
    backlog_finding_with_promotion, batch_promote_findings, promotion_overrides_for_finding,
    triage_finding,
};
use crate::item::{next_sort_key, next_sort_key_after, pending_approval_state};
use crate::job::{DispatchJobCommand, dispatch_job};

#[derive(Clone, Debug)]
pub struct TriageFindingCommand {
    pub finding_id: FindingId,
    pub triage_state: FindingTriageState,
    pub triage_note: Option<String>,
    pub linked_item_id: Option<ItemId>,
    pub target_ref: Option<GitRef>,
    pub approval_policy: Option<ApprovalPolicy>,
}

#[derive(Clone, Debug)]
pub struct PromoteFindingCommand {
    pub finding_id: FindingId,
    pub target_ref: Option<GitRef>,
    pub approval_policy: Option<ApprovalPolicy>,
    pub dispatch_immediately: bool,
}

#[derive(Clone, Debug)]
pub struct BatchPromoteFindingsCommand {
    pub project_id: ProjectId,
    pub finding_ids: Vec<FindingId>,
}

#[derive(Debug)]
pub struct AppliedFindingTriage {
    pub finding: Finding,
    pub linked_item: Option<Item>,
    pub linked_revision: Option<ItemRevision>,
    pub auto_dispatch_result: Option<Result<Option<Job>, UseCaseError>>,
}

#[derive(Debug)]
pub enum PromoteFindingLaunch {
    NotRequested,
    Dispatched(Box<Job>),
    DispatchFailed(UseCaseError),
    NoDispatchableStep,
}

#[derive(Debug)]
pub struct PromoteFindingOutput {
    pub item: Item,
    pub current_revision: ItemRevision,
    pub finding: Finding,
    pub launch: PromoteFindingLaunch,
}

pub async fn apply_finding_triage<R, I, L>(
    repo: &R,
    infra: &I,
    project_locks: &L,
    command: TriageFindingCommand,
) -> Result<AppliedFindingTriage, UseCaseError>
where
    R: FindingRepository
        + ItemRepository
        + RevisionRepository
        + ProjectRepository
        + JobRepository
        + ActivityRepository
        + WorkspaceRepository
        + GitOperationRepository
        + ConvergenceRepository,
    I: ApplicationInfraPort + DispatchInfraPort,
    L: ProjectMutationLockPort,
{
    let finding = <R as FindingRepository>::get(repo, command.finding_id)
        .await
        .map_err(map_finding_get_error)?;
    let source_item = <R as ItemRepository>::get(repo, finding.source_item_id)
        .await
        .map_err(map_item_get_error)?;
    let source_revision =
        <R as RevisionRepository>::get(repo, finding.source_item_revision_id).await?;
    let project = <R as ProjectRepository>::get(repo, source_item.project_id)
        .await
        .map_err(map_project_get_error)?;
    let _guard = project_locks.acquire_project_mutation(project.id).await;

    let detached_origin_item_id =
        find_detached_origin_item_id(repo, &finding, command.linked_item_id).await?;

    let mut applied = match command.triage_state {
        FindingTriageState::Backlog => {
            ensure_finding_subject_reachable(infra, &project, &finding).await?;
            if let Some(linked_item_id) = command.linked_item_id {
                let linked_item =
                    load_linked_item_for_finding(repo, &source_item, linked_item_id).await?;
                if linked_item.id == source_item.id {
                    return Err(UseCaseError::InvalidFindingTriage(
                        "backlog triage must link to a different item".into(),
                    ));
                }
                let triaged = triage_finding(
                    &finding,
                    TriageFindingInput {
                        triage_state: FindingTriageState::Backlog,
                        triage_note: command.triage_note,
                        linked_item_id: Some(linked_item.id),
                    },
                )?;
                repo.triage_with_origin_detached(&triaged, detached_origin_item_id)
                    .await?;
                AppliedFindingTriage {
                    finding: triaged,
                    linked_item: Some(linked_item),
                    linked_revision: None,
                    auto_dispatch_result: None,
                }
            } else {
                let overrides = BacklogFindingOverrides {
                    target_ref: command.target_ref,
                    approval_policy: command.approval_policy,
                };
                let source_jobs = <R as JobRepository>::list_by_item(repo, source_item.id).await?;
                let promotion_overrides = promotion_overrides_for_finding(&finding, &source_jobs);
                let sort_key = next_project_sort_key(repo, source_item.project_id).await?;
                let (linked_item, linked_revision, triaged) = backlog_finding_with_promotion(
                    &finding,
                    &source_item,
                    &source_revision,
                    overrides,
                    sort_key,
                    command.triage_note,
                    promotion_overrides,
                )?;
                repo.link_backlog(
                    &triaged,
                    &linked_item,
                    &linked_revision,
                    detached_origin_item_id,
                )
                .await?;
                AppliedFindingTriage {
                    finding: triaged,
                    linked_item: Some(linked_item),
                    linked_revision: Some(linked_revision),
                    auto_dispatch_result: None,
                }
            }
        }
        FindingTriageState::Duplicate => {
            let linked_item_id = command.linked_item_id.ok_or_else(|| {
                UseCaseError::InvalidFindingTriage(
                    "duplicate triage requires linked_item_id".into(),
                )
            })?;
            let linked_item =
                load_linked_item_for_finding(repo, &source_item, linked_item_id).await?;
            if linked_item.id == source_item.id {
                return Err(UseCaseError::InvalidFindingTriage(
                    "duplicate triage must link to a different item".into(),
                ));
            }
            let triaged = triage_finding(
                &finding,
                TriageFindingInput {
                    triage_state: FindingTriageState::Duplicate,
                    triage_note: command.triage_note,
                    linked_item_id: Some(linked_item.id),
                },
            )?;
            repo.triage_with_origin_detached(&triaged, detached_origin_item_id)
                .await?;
            AppliedFindingTriage {
                finding: triaged,
                linked_item: Some(linked_item),
                linked_revision: None,
                auto_dispatch_result: None,
            }
        }
        _ => {
            let triaged = triage_finding(
                &finding,
                TriageFindingInput {
                    triage_state: command.triage_state,
                    triage_note: command.triage_note,
                    linked_item_id: command.linked_item_id,
                },
            )?;
            repo.triage_with_origin_detached(&triaged, detached_origin_item_id)
                .await?;
            AppliedFindingTriage {
                finding: triaged,
                linked_item: None,
                linked_revision: None,
                auto_dispatch_result: None,
            }
        }
    };

    maybe_enter_approval_after_finding_triage(
        repo,
        &source_item,
        &source_revision,
        &applied.finding,
    )
    .await?;
    maybe_cleanup_investigation_ref(
        repo,
        repo,
        repo,
        infra,
        source_item.project_id,
        &applied.finding,
    )
    .await?;

    append_activity(
        repo,
        source_item.project_id,
        ActivityEventType::FindingTriaged,
        ActivitySubject::Finding(applied.finding.id),
        serde_json::json!({
            "item_id": source_item.id,
            "triage_state": applied.finding.triage.state(),
            "linked_item_id": applied.finding.triage.linked_item_id(),
        }),
    )
    .await?;

    if step::is_closure_relevant_review_step(applied.finding.source_step_id) {
        applied.auto_dispatch_result = Some(
            crate::item_commands::auto_dispatch_projected_review_job(
                repo,
                infra,
                &project,
                source_item.id,
            )
            .await,
        );
    }

    Ok(applied)
}

pub async fn promote_finding<R, I, L>(
    repo: &R,
    infra: &I,
    project_locks: &L,
    command: PromoteFindingCommand,
) -> Result<PromoteFindingOutput, UseCaseError>
where
    R: FindingRepository
        + ItemRepository
        + RevisionRepository
        + ProjectRepository
        + JobRepository
        + ActivityRepository
        + WorkspaceRepository
        + GitOperationRepository
        + ConvergenceRepository,
    I: ApplicationInfraPort + DispatchInfraPort,
    L: ProjectMutationLockPort,
{
    if command.dispatch_immediately {
        let finding = <R as FindingRepository>::get(repo, command.finding_id)
            .await
            .map_err(map_finding_get_error)?;
        if !supports_promote_and_launch(&finding) {
            return Err(UseCaseError::InvalidFindingTriage(
                "dispatch_immediately is only supported for investigation findings".into(),
            ));
        }
    }

    let applied = apply_finding_triage(
        repo,
        infra,
        project_locks,
        TriageFindingCommand {
            finding_id: command.finding_id,
            triage_state: FindingTriageState::Backlog,
            triage_note: None,
            linked_item_id: None,
            target_ref: command.target_ref,
            approval_policy: command.approval_policy,
        },
    )
    .await?;
    let item = applied.linked_item.ok_or_else(|| {
        UseCaseError::Internal("Backlog promotion did not create a linked item".into())
    })?;
    let current_revision = applied.linked_revision.ok_or_else(|| {
        UseCaseError::Internal("Backlog promotion did not create a linked revision".into())
    })?;

    let launch = if command.dispatch_immediately {
        let project = <R as ProjectRepository>::get(repo, item.project_id)
            .await
            .map_err(map_project_get_error)?;
        let _guard = project_locks.acquire_project_mutation(project.id).await;
        match dispatch_projected_item_job(repo, infra, &project, item.id, "operator").await {
            Ok(Some(job)) => PromoteFindingLaunch::Dispatched(Box::new(job)),
            Ok(None) => PromoteFindingLaunch::NoDispatchableStep,
            Err(error) => PromoteFindingLaunch::DispatchFailed(error),
        }
    } else {
        PromoteFindingLaunch::NotRequested
    };

    Ok(PromoteFindingOutput {
        item,
        current_revision,
        finding: applied.finding,
        launch,
    })
}

pub async fn batch_promote_findings_command<R, L>(
    repo: &R,
    project_locks: &L,
    command: BatchPromoteFindingsCommand,
) -> Result<BatchPromoteOutput, UseCaseError>
where
    R: ProjectRepository
        + FindingRepository
        + ItemRepository
        + RevisionRepository
        + JobRepository
        + ActivityRepository,
    L: ProjectMutationLockPort,
{
    if command.finding_ids.is_empty() {
        return Ok(BatchPromoteOutput {
            promoted: vec![],
            skipped: vec![],
        });
    }

    let _project = <R as ProjectRepository>::get(repo, command.project_id)
        .await
        .map_err(map_project_get_error)?;
    let _guard = project_locks
        .acquire_project_mutation(command.project_id)
        .await;
    let first_finding = <R as FindingRepository>::get(repo, command.finding_ids[0])
        .await
        .map_err(map_finding_get_error)?;
    let source_item = <R as ItemRepository>::get(repo, first_finding.source_item_id)
        .await
        .map_err(map_item_get_error)?;
    if source_item.project_id != command.project_id {
        return Err(UseCaseError::ItemNotFound);
    }
    let source_revision =
        <R as RevisionRepository>::get(repo, first_finding.source_item_revision_id).await?;
    let findings = <R as FindingRepository>::list_by_item(repo, source_item.id).await?;
    let source_jobs = <R as JobRepository>::list_by_item(repo, source_item.id).await?;

    let base_sort_key = next_project_sort_key(repo, command.project_id).await?;
    let mut last_sort_key = base_sort_key;
    let mut sort_key_fn = || {
        let key = next_sort_key_after(Some(&last_sort_key));
        last_sort_key = key.clone();
        key
    };

    let output = batch_promote_findings(
        &findings,
        &source_item,
        &source_revision,
        &source_jobs,
        BatchPromoteInput {
            finding_ids: command.finding_ids,
        },
        &mut sort_key_fn,
    )?;

    for result in &output.promoted {
        repo.link_backlog(
            &result.triaged_finding,
            &result.linked_item,
            &result.linked_revision,
            None,
        )
        .await?;

        append_activity(
            repo,
            command.project_id,
            ActivityEventType::FindingTriaged,
            ActivitySubject::Finding(result.finding_id),
            serde_json::json!({
                "item_id": source_item.id,
                "triage_state": result.triaged_finding.triage.state(),
                "linked_item_id": result.linked_item.id,
                "batch": true,
            }),
        )
        .await?;
    }

    Ok(output)
}

async fn dispatch_projected_item_job<R, I>(
    repo: &R,
    infra: &I,
    project: &Project,
    item_id: ItemId,
    dispatch_origin: &'static str,
) -> Result<Option<Job>, UseCaseError>
where
    R: ItemRepository
        + RevisionRepository
        + JobRepository
        + FindingRepository
        + ConvergenceRepository
        + WorkspaceRepository
        + GitOperationRepository
        + ActivityRepository,
    I: ApplicationInfraPort + DispatchInfraPort,
{
    let item = <R as ItemRepository>::get(repo, item_id)
        .await
        .map_err(map_item_get_error)?;
    if item.project_id != project.id {
        return Err(UseCaseError::ItemNotFound);
    }

    let snapshot = load_item_runtime_snapshot(repo, infra, project.id, &item).await?;
    let evaluation = Evaluator::new().evaluate(
        &item,
        &snapshot.current_revision,
        &snapshot.jobs,
        &snapshot.findings,
        &snapshot.convergences,
    );
    let Some(step_id) = evaluation.dispatchable_step_id else {
        return Ok(None);
    };
    let job = dispatch_job(
        &item,
        &snapshot.current_revision,
        &snapshot.jobs,
        &snapshot.findings,
        &snapshot.convergences,
        DispatchJobCommand {
            step_id: Some(step_id),
        },
    )?;
    let prepared = prepare_and_persist_dispatched_job(
        repo,
        repo,
        repo,
        repo,
        infra,
        project,
        &item,
        &snapshot.current_revision,
        &snapshot.jobs,
        job,
        DispatchActivityContext {
            dispatch_origin: Some(dispatch_origin),
            supersedes_job_id: None,
            retry_no: None,
        },
    )
    .await?;

    Ok(Some(prepared.job))
}

async fn find_detached_origin_item_id<R>(
    repo: &R,
    finding: &Finding,
    next_linked_item_id: Option<ItemId>,
) -> Result<Option<ItemId>, UseCaseError>
where
    R: ItemRepository,
{
    let Some(current_linked_item_id) = finding.triage.linked_item_id() else {
        return Ok(None);
    };
    if finding.triage.state() != FindingTriageState::Backlog {
        return Ok(None);
    }
    if next_linked_item_id == Some(current_linked_item_id) {
        return Ok(None);
    }

    let linked_item = <R as ItemRepository>::get(repo, current_linked_item_id)
        .await
        .map_err(map_item_get_error)?;
    if linked_item.origin.is_promoted_finding()
        && linked_item.origin.finding_id() == Some(finding.id)
    {
        Ok(Some(linked_item.id))
    } else {
        Ok(None)
    }
}

async fn load_linked_item_for_finding<R>(
    repo: &R,
    source_item: &Item,
    linked_item_id: ItemId,
) -> Result<Item, UseCaseError>
where
    R: ItemRepository,
{
    let linked_item = <R as ItemRepository>::get(repo, linked_item_id)
        .await
        .map_err(|error| match error {
            RepositoryError::NotFound => UseCaseError::LinkedItemNotFound,
            other => UseCaseError::Repository(other),
        })?;

    if linked_item.project_id != source_item.project_id {
        return Err(UseCaseError::LinkedItemProjectMismatch);
    }

    Ok(linked_item)
}

async fn maybe_enter_approval_after_finding_triage<R>(
    repo: &R,
    source_item: &Item,
    source_revision: &ItemRevision,
    finding: &Finding,
) -> Result<(), UseCaseError>
where
    R: JobRepository + FindingRepository + ItemRepository + ActivityRepository,
{
    if finding.source_step_id != StepId::ValidateIntegrated
        || source_item.current_revision_id != source_revision.id
    {
        return Ok(());
    }

    let jobs = <R as JobRepository>::list_by_item(repo, source_item.id).await?;
    let latest_closure_findings_job =
        crate::dispatch::latest_closure_findings_job(&jobs, source_revision.id);

    let Some(latest_job) = latest_closure_findings_job else {
        return Ok(());
    };
    if latest_job.id != finding.source_job_id {
        return Ok(());
    }

    let findings = <R as FindingRepository>::list_by_item(repo, source_item.id).await?;
    let latest_job_findings = findings
        .iter()
        .filter(|row| row.source_item_revision_id == source_revision.id)
        .filter(|row| row.source_job_id == latest_job.id)
        .collect::<Vec<_>>();

    if latest_job_findings.is_empty()
        || latest_job_findings.iter().any(|row| {
            row.triage.is_unresolved() || row.triage.state() == FindingTriageState::FixNow
        })
    {
        return Ok(());
    }

    let mut item = <R as ItemRepository>::get(repo, source_item.id)
        .await
        .map_err(map_item_get_error)?;
    let next_approval_state = pending_approval_state(source_revision.approval_policy);
    if item.approval_state != next_approval_state {
        item.approval_state = next_approval_state;
        item.updated_at = Utc::now();
        <R as ItemRepository>::update(repo, &item).await?;

        if next_approval_state == ApprovalState::Pending {
            append_activity(
                repo,
                item.project_id,
                ActivityEventType::ApprovalRequested,
                ActivitySubject::Item(item.id),
                serde_json::json!({ "source": "finding_triage" }),
            )
            .await?;
        }
    }

    Ok(())
}

async fn ensure_finding_subject_reachable<I>(
    infra: &I,
    project: &Project,
    finding: &Finding,
) -> Result<(), UseCaseError>
where
    I: ApplicationInfraPort,
{
    let head_reachable = infra
        .is_commit_reachable_from_project(project, &finding.source_subject_head_commit_oid)
        .await?;

    if !head_reachable {
        return Err(UseCaseError::FindingSubjectUnreachable);
    }

    if finding.source_subject_kind == FindingSubjectKind::Integrated {
        let Some(base_commit_oid) = finding.source_subject_base_commit_oid.as_ref() else {
            return Err(UseCaseError::FindingSubjectUnreachable);
        };
        let base_reachable = infra
            .is_commit_reachable_from_project(project, base_commit_oid)
            .await?;

        if !base_reachable {
            return Err(UseCaseError::FindingSubjectUnreachable);
        }
    }

    Ok(())
}

fn supports_promote_and_launch(finding: &Finding) -> bool {
    finding.investigation.is_some()
        || matches!(
            finding.source_step_id,
            StepId::InvestigateItem | StepId::InvestigateProject | StepId::ReinvestigateProject
        )
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

fn map_finding_get_error(error: RepositoryError) -> UseCaseError {
    match error {
        RepositoryError::NotFound => UseCaseError::FindingNotFound,
        other => UseCaseError::Repository(other),
    }
}

async fn next_project_sort_key<R>(repo: &R, project_id: ProjectId) -> Result<String, UseCaseError>
where
    R: ItemRepository,
{
    let items = <R as ItemRepository>::list_by_project(repo, project_id).await?;
    Ok(next_sort_key(&items))
}

async fn append_activity<A>(
    activity_repo: &A,
    project_id: ProjectId,
    event_type: ActivityEventType,
    subject: ActivitySubject,
    payload: serde_json::Value,
) -> Result<(), UseCaseError>
where
    A: ActivityRepository,
{
    activity_repo
        .append(&Activity {
            id: ActivityId::new(),
            project_id,
            event_type,
            subject,
            payload,
            created_at: Utc::now(),
        })
        .await?;
    Ok(())
}
