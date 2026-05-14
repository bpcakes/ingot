use std::collections::HashMap;

use ingot_domain::convergence::{Convergence, ConvergenceStatus};
use ingot_domain::finding::Finding;
use ingot_domain::git_operation::ConvergenceConflictMetadata;
use ingot_domain::ids::{ConvergenceId, ItemId, ProjectId};
use ingot_domain::item::Item;
use ingot_domain::project::Project;
use ingot_usecases::UseCaseError;
use ingot_usecases::application::{
    FinalizationPhase, FinalizationStatus, ItemProjection, ItemRuntimeSnapshot, QueueStatus,
};
use ingot_usecases::finding::parse_revision_context_summary;
use ingot_workflow::{Evaluation, Evaluator};

use super::app::AppState;
use super::support::errors::{repo_to_internal, repo_to_item, repo_to_project};
use super::types::*;
use crate::error::ApiError;

pub(super) async fn load_item_runtime_snapshot(
    state: &AppState,
    project_id: ProjectId,
    item: &Item,
) -> Result<ItemRuntimeSnapshot, ingot_usecases::UseCaseError> {
    ingot_usecases::application::load_item_runtime_snapshot(
        state.db(),
        &state.infra(),
        project_id,
        item,
    )
    .await
}

pub(super) async fn load_item_detail(
    state: &AppState,
    project_id: ProjectId,
    item_id: ItemId,
) -> Result<ItemDetailResponse, ApiError> {
    let db = state.db();
    let item = db.get_item(item_id).await.map_err(repo_to_item)?;
    if item.project_id != project_id {
        return Err(UseCaseError::ItemNotFound.into());
    }
    let project = db
        .get_project(item.project_id)
        .await
        .map_err(repo_to_project)?;
    let snapshot = load_item_runtime_snapshot(state, project.id, &item).await?;
    let revision_history = db
        .list_revisions_by_item(item.id)
        .await
        .map_err(repo_to_internal)?;
    let workspaces = db
        .list_workspaces_by_item(item.id)
        .await
        .map_err(repo_to_internal)?;
    let revision_context = db
        .get_revision_context(item.current_revision_id)
        .await
        .map_err(repo_to_internal)?;
    let revision_context_summary = parse_revision_context_summary(revision_context.as_ref());
    let evaluator = Evaluator::new();
    let (evaluation, finalization, queue) =
        evaluate_item_snapshot(state, &project, &item, &snapshot, &evaluator).await?;
    let diagnostics = evaluation.diagnostics.clone();
    let ItemRuntimeSnapshot {
        current_revision,
        jobs,
        findings,
        convergences,
    } = snapshot;
    let linked_finding_items = load_linked_finding_items(state, &project, &findings).await?;
    let mut convergence_conflicts = load_convergence_conflicts(state, &convergences).await?;

    Ok(ItemDetailResponse {
        item,
        workflow_presentations: ingot_workflow::WORKFLOW_PRESENTATIONS,
        execution_mode: project.execution_mode,
        current_revision,
        evaluation,
        finalization,
        queue,
        revision_history,
        jobs,
        findings,
        linked_finding_items,
        workspaces,
        convergences: convergences
            .into_iter()
            .map(|convergence| {
                let conflict = convergence_conflicts.remove(&convergence.id);
                convergence_response(convergence, conflict)
            })
            .collect(),
        revision_context_summary,
        diagnostics,
    })
}

async fn load_convergence_conflicts(
    state: &AppState,
    convergences: &[Convergence],
) -> Result<HashMap<ConvergenceId, ConvergenceConflictMetadata>, ApiError> {
    let mut conflicts = HashMap::new();
    let convergence_ids = convergences
        .iter()
        .filter(|convergence| convergence.state.status() == ConvergenceStatus::Conflicted)
        .map(|convergence| convergence.id)
        .collect::<Vec<_>>();
    if convergence_ids.is_empty() {
        return Ok(conflicts);
    }

    let operations = state
        .db()
        .list_latest_failed_prepare_for_convergences(&convergence_ids)
        .await
        .map_err(repo_to_internal)?;

    for operation in operations {
        let ingot_domain::git_operation::GitOperationEntityRef::Convergence(convergence_id) =
            operation.entity
        else {
            continue;
        };
        let Some(conflict) = operation
            .payload
            .replay_metadata()
            .and_then(|metadata| metadata.conflict.clone())
        else {
            continue;
        };

        conflicts.insert(convergence_id, conflict);
    }

    Ok(conflicts)
}

async fn load_linked_finding_items(
    state: &AppState,
    project: &Project,
    findings: &[Finding],
) -> Result<Vec<LinkedFindingItemSummary>, ApiError> {
    let mut linked_items = Vec::new();

    for finding in findings {
        let Some(linked_item_id) = finding.triage.linked_item_id() else {
            continue;
        };

        let item = state
            .db()
            .get_item(linked_item_id)
            .await
            .map_err(repo_to_internal)?;
        if item.project_id != project.id
            || !item.origin.is_promoted_finding()
            || item.origin.finding_id() != Some(finding.id)
        {
            continue;
        }

        let snapshot = load_item_runtime_snapshot(state, project.id, &item).await?;
        let evaluator = Evaluator::new();
        let (evaluation, _, _) =
            evaluate_item_snapshot(state, project, &item, &snapshot, &evaluator).await?;
        let title = snapshot.current_revision.title.clone();
        let job_count = snapshot.jobs.len();

        linked_items.push(LinkedFindingItemSummary {
            finding_id: finding.id,
            item,
            title,
            board_status: evaluation.board_status,
            job_count,
        });
    }

    Ok(linked_items)
}

pub(super) async fn evaluate_item_snapshot(
    state: &AppState,
    project: &Project,
    item: &Item,
    snapshot: &ItemRuntimeSnapshot,
    evaluator: &Evaluator,
) -> Result<(Evaluation, FinalizationStatusResponse, QueueStatusResponse), ApiError> {
    let ItemProjection {
        evaluation,
        finalization,
        queue,
    } = ingot_usecases::application::evaluate_item_snapshot(
        state.db(),
        project,
        item,
        snapshot,
        evaluator,
    )
    .await?;

    Ok((
        evaluation,
        finalization_status_response(finalization),
        queue_status_response(queue),
    ))
}

fn convergence_response(
    convergence: Convergence,
    conflict: Option<ConvergenceConflictMetadata>,
) -> ConvergenceResponse {
    let conflict_summary = (convergence.state.status() == ConvergenceStatus::Conflicted)
        .then(|| convergence.state.conflict_summary().map(str::to_owned))
        .flatten();
    let failure_summary = (convergence.state.status() == ConvergenceStatus::Failed)
        .then(|| convergence.state.conflict_summary().map(str::to_owned))
        .flatten();
    ConvergenceResponse {
        id: convergence.id,
        status: convergence.state.status(),
        input_target_commit_oid: convergence.state.input_target_commit_oid().cloned(),
        prepared_commit_oid: convergence.state.prepared_commit_oid().cloned(),
        final_target_commit_oid: convergence.state.final_target_commit_oid().cloned(),
        conflict_summary,
        failure_summary,
        conflict: (convergence.state.status() == ConvergenceStatus::Conflicted)
            .then(|| conflict.map(conflict_response))
            .flatten(),
        target_head_valid: convergence.target_head_valid.unwrap_or(true),
    }
}

fn conflict_response(conflict: ConvergenceConflictMetadata) -> ConvergenceConflictResponse {
    ConvergenceConflictResponse {
        failed_source_commit_oid: conflict.failed_source_commit_oid,
        git_error: conflict.git_error,
        total_file_count: conflict.total_file_count,
        files_truncated: conflict.files_truncated,
        files: conflict
            .files
            .into_iter()
            .map(|file| ConvergenceConflictFileResponse {
                path: file.path,
                stages: file.stages,
                excerpt: file.excerpt,
            })
            .collect(),
    }
}

fn finalization_status_response(finalization: FinalizationStatus) -> FinalizationStatusResponse {
    FinalizationStatusResponse {
        phase: match finalization.phase {
            FinalizationPhase::None => FinalizationPhaseResponse::None,
            FinalizationPhase::ReadyToFinalize => FinalizationPhaseResponse::ReadyToFinalize,
            FinalizationPhase::TargetRefAdvanced => FinalizationPhaseResponse::TargetRefAdvanced,
        },
        checkout_adoption_state: finalization.checkout_adoption_state,
        checkout_adoption_message: finalization.checkout_adoption_message,
        final_target_commit_oid: finalization.final_target_commit_oid,
    }
}

fn queue_status_response(queue: QueueStatus) -> QueueStatusResponse {
    QueueStatusResponse {
        state: queue.state,
        position: queue.position,
        lane_owner_item_id: queue.lane_owner_item_id,
        lane_target_ref: queue.lane_target_ref,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::router::test_helpers::test_app_state;
    use chrono::Utc;
    use ingot_domain::commit_oid::CommitOid;
    use ingot_domain::git_operation::{
        ConvergenceConflictFile, ConvergenceConflictMetadata, ConvergenceConflictStage,
    };
    use ingot_domain::ids::{ItemId, ItemRevisionId, ProjectId};
    use ingot_domain::test_support::{ConvergenceBuilder, ProjectBuilder};
    use ingot_test_support::git::{
        git_output as support_git_output, run_git as support_git,
        temp_git_repo as support_temp_git_repo, write_file as support_write_file,
    };
    use uuid::Uuid;

    fn temp_git_repo() -> PathBuf {
        support_temp_git_repo("ingot-http-api")
    }

    fn git(path: &std::path::Path, args: &[&str]) {
        support_git(path, args);
    }

    fn git_output(path: &std::path::Path, args: &[&str]) -> String {
        support_git_output(path, args)
    }

    fn write_file(path: &std::path::Path, contents: &str) {
        support_write_file(path, contents);
    }

    #[tokio::test]
    async fn target_head_valid_tracks_ref_movement() {
        let state = test_app_state().await;
        let repo = temp_git_repo();
        let project = ProjectBuilder::new(&repo)
            .id(ProjectId::from_uuid(Uuid::nil()))
            .created_at(Utc::now())
            .build();
        state
            .db()
            .create_project(&project)
            .await
            .expect("create project");
        let first = git_output(&repo, &["rev-parse", "HEAD"]);
        let mut convergence = ConvergenceBuilder::new(
            project.id,
            ItemId::from_uuid(Uuid::nil()),
            ItemRevisionId::from_uuid(Uuid::nil()),
        )
        .target_head_valid(true)
        .created_at(Utc::now())
        .input_target_commit_oid(first.clone())
        .build();
        convergence.target_ref = "refs/heads/main".into();

        let mut valid = vec![convergence.clone()];
        ingot_usecases::application::hydrate_convergence_validity(
            &state.infra(),
            project.id,
            &mut valid,
        )
        .await
        .expect("compute validity");
        assert_eq!(valid[0].target_head_valid, Some(true));

        write_file(&repo.join("tracked.txt"), "next");
        git(&repo, &["add", "tracked.txt"]);
        git(&repo, &["commit", "-m", "next"]);

        let mut stale = vec![convergence];
        ingot_usecases::application::hydrate_convergence_validity(
            &state.infra(),
            project.id,
            &mut stale,
        )
        .await
        .expect("compute stale validity");
        assert_eq!(stale[0].target_head_valid, Some(false));
    }

    #[test]
    fn convergence_response_includes_conflict_summary() {
        let convergence = ConvergenceBuilder::new(
            ProjectId::from_uuid(Uuid::nil()),
            ItemId::from_uuid(Uuid::nil()),
            ItemRevisionId::from_uuid(Uuid::nil()),
        )
        .status(ingot_domain::convergence::ConvergenceStatus::Conflicted)
        .conflict_summary("tracked.txt conflicted")
        .build();

        let response = super::convergence_response(convergence, None);

        assert_eq!(
            response.conflict_summary.as_deref(),
            Some("tracked.txt conflicted")
        );
        assert!(response.conflict.is_none());
    }

    #[test]
    fn convergence_response_includes_conflict_metadata() {
        let convergence = ConvergenceBuilder::new(
            ProjectId::from_uuid(Uuid::nil()),
            ItemId::from_uuid(Uuid::nil()),
            ItemRevisionId::from_uuid(Uuid::nil()),
        )
        .status(ingot_domain::convergence::ConvergenceStatus::Conflicted)
        .conflict_summary("tracked.txt conflicted")
        .build();
        let conflict = ConvergenceConflictMetadata {
            failed_source_commit_oid: CommitOid::new("0123456789abcdef"),
            git_error: "git failed".into(),
            total_file_count: 1,
            files_truncated: false,
            files: vec![ConvergenceConflictFile {
                path: "tracked.txt".into(),
                stages: vec![
                    ConvergenceConflictStage::Ours,
                    ConvergenceConflictStage::Theirs,
                ],
                excerpt: Some("<<<<<<< ours".into()),
            }],
        };

        let response = super::convergence_response(convergence, Some(conflict));
        let conflict = response.conflict.expect("conflict metadata");

        assert_eq!(conflict.total_file_count, 1);
        assert_eq!(conflict.files[0].path, "tracked.txt");
        assert_eq!(
            conflict.files[0].stages,
            vec![
                ConvergenceConflictStage::Ours,
                ConvergenceConflictStage::Theirs
            ]
        );
    }

    #[test]
    fn convergence_response_exposes_failed_summary_without_labeling_it_as_conflict() {
        let mut convergence = ConvergenceBuilder::new(
            ProjectId::from_uuid(Uuid::nil()),
            ItemId::from_uuid(Uuid::nil()),
            ItemRevisionId::from_uuid(Uuid::nil()),
        )
        .status(ingot_domain::convergence::ConvergenceStatus::Running)
        .build();
        convergence.transition_to_failed(Some("prepare convergence failed".into()), Utc::now());

        let response = super::convergence_response(convergence, None);

        assert_eq!(
            response.status,
            ingot_domain::convergence::ConvergenceStatus::Failed
        );
        assert_eq!(response.conflict_summary, None);
        assert_eq!(
            response.failure_summary.as_deref(),
            Some("prepare convergence failed")
        );
        assert!(response.conflict.is_none());
    }
}
