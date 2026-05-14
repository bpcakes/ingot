// Convergence preparation, finalization, and invalidation.

use std::path::PathBuf;

use chrono::Utc;
use ingot_domain::activity::{Activity, ActivityEventType, ActivitySubject};
use ingot_domain::commit_oid::CommitOid;
use ingot_domain::convergence::{Convergence, ConvergenceStatus, PrepareFailureKind};
use ingot_domain::convergence_queue::{ConvergenceQueueEntry, ConvergenceQueueEntryStatus};
use ingot_domain::git_operation::{
    ConvergenceConflictFile, ConvergenceConflictMetadata, ConvergenceReplayMetadata, GitOperation,
    GitOperationEntityRef, GitOperationStatus, OperationPayload,
    truncate_convergence_conflict_git_error,
};
use ingot_domain::git_ref::GitRef;
use ingot_domain::ids::{ActivityId, GitOperationId, WorkspaceId};
use ingot_domain::item::{ApprovalState, Escalation, EscalationReason};
use ingot_domain::job::Job;
use ingot_domain::ports::{
    ItemEscalationPatch, PrepareConvergenceFailureMutation, PrepareConvergenceFailureRepository,
    ProjectMutationLockPort,
};
use ingot_domain::project::Project;
use ingot_domain::revision::ItemRevision;
use ingot_domain::step_id::StepId;
use ingot_domain::workspace::{
    RetentionPolicy, Workspace, WorkspaceCommitState, WorkspaceKind, WorkspaceState,
    WorkspaceStrategy,
};
use ingot_git::commands::{git, resolve_ref_oid};
use ingot_git::commit::{
    ConvergenceCommitTrailers, abort_cherry_pick, cherry_pick_no_commit,
    collect_convergence_conflict_files, commit_message, list_commits_oldest_first,
    working_tree_has_changes,
};
use ingot_git::project_repo::{CheckoutSyncStatus, checkout_sync_status};
use ingot_usecases::convergence::{FinalizePreparedTrigger, finalize_prepared_convergence};
use ingot_usecases::job::{DispatchJobCommand, dispatch_job};
use ingot_workspace::provision_integration_workspace;
use tracing::{info, warn};

use crate::{JobDispatcher, RuntimeError, RuntimeFinalizePort, usecase_to_runtime_error};

const MAX_PREPARE_FAILURE_SUMMARY_BYTES: usize = 2 * 1024;

impl JobDispatcher {
    pub(crate) async fn auto_finalize_prepared_convergence(
        &self,
        project_id: ingot_domain::ids::ProjectId,
        item_id: ingot_domain::ids::ItemId,
    ) -> Result<bool, RuntimeError> {
        let _guard = self
            .project_locks
            .acquire_project_mutation(project_id)
            .await;
        let project = self.db.get_project(project_id).await?;
        let paths = self.refresh_project_mirror(&project).await?;
        let item = self.db.get_item(item_id).await?;
        let revision = self.db.get_revision(item.current_revision_id).await?;
        let jobs = self.db.list_jobs_by_item(item.id).await?;
        let findings = self.db.list_findings_by_item(item.id).await?;
        let convergences = self
            .hydrate_convergences(&project, self.db.list_convergences_by_item(item.id).await?)
            .await?;
        let queue_entry = self
            .db
            .find_active_queue_entry_for_revision(revision.id)
            .await?;
        if !ingot_usecases::convergence::should_auto_finalize_prepared_convergence(
            &item,
            &revision,
            &jobs,
            &findings,
            &convergences,
            queue_entry.as_ref(),
        ) {
            return Ok(false);
        }

        let convergence = convergences
            .into_iter()
            .find(|convergence| {
                convergence.item_revision_id == revision.id
                    && convergence.state.status() == ConvergenceStatus::Prepared
            })
            .ok_or_else(|| RuntimeError::InvalidState("prepared convergence missing".into()))?;
        let prepared_commit_oid = convergence
            .state
            .prepared_commit_oid()
            .map(ToOwned::to_owned)
            .ok_or_else(|| RuntimeError::InvalidState("prepared commit missing".into()))?;
        let input_target_commit_oid = convergence
            .state
            .input_target_commit_oid()
            .map(ToOwned::to_owned)
            .ok_or_else(|| RuntimeError::InvalidState("input target commit missing".into()))?;
        let current_target_oid =
            resolve_ref_oid(paths.mirror_git_dir.as_path(), &convergence.target_ref).await?;
        let target_valid = current_target_oid.as_ref() == Some(&prepared_commit_oid)
            || current_target_oid.as_ref() == Some(&input_target_commit_oid);
        if !target_valid {
            return Ok(false);
        }

        match finalize_prepared_convergence(
            &RuntimeFinalizePort {
                dispatcher: self.clone(),
            },
            FinalizePreparedTrigger::SystemCommand,
            &project,
            &item,
            &revision,
            &convergence,
            queue_entry
                .as_ref()
                .expect("queue head already validated for auto-finalize"),
        )
        .await
        {
            Ok(()) => {}
            Err(ingot_usecases::UseCaseError::ProtocolViolation(_)) => return Ok(false),
            Err(error) => return Err(usecase_to_runtime_error(error)),
        }

        info!(item_id = %item.id, convergence_id = %convergence.id, "auto-finalized prepared convergence");
        Ok(true)
    }

    pub(crate) async fn invalidate_prepared_convergence(
        &self,
        project_id: ingot_domain::ids::ProjectId,
        item_id: ingot_domain::ids::ItemId,
    ) -> Result<(), RuntimeError> {
        let _guard = self
            .project_locks
            .acquire_project_mutation(project_id)
            .await;
        let project = self.db.get_project(project_id).await?;
        let mut item = self.db.get_item(item_id).await?;
        let revision = self.db.get_revision(item.current_revision_id).await?;
        let jobs = self.db.list_jobs_by_item(item.id).await?;
        let findings = self.db.list_findings_by_item(item.id).await?;
        let convergences = self
            .hydrate_convergences(&project, self.db.list_convergences_by_item(item.id).await?)
            .await?;
        if !ingot_usecases::convergence::should_invalidate_prepared_convergence(
            &item,
            &revision,
            &jobs,
            &findings,
            &convergences,
        ) {
            return Ok(());
        }

        let invalidated = ingot_usecases::convergence::invalidate_prepared_convergence(
            &self.db,
            &mut item,
            &revision,
            &convergences,
        )
        .await
        .map_err(|e| RuntimeError::InvalidState(e.to_string()))?;

        if invalidated {
            info!(item_id = %item.id, "invalidated stale prepared convergence");
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn fail_prepare_convergence_attempt(
        &self,
        project: &Project,
        item: &ingot_domain::item::Item,
        revision: &ItemRevision,
        queue_entry: &ConvergenceQueueEntry,
        integration_workspace: &mut Workspace,
        convergence: &mut Convergence,
        operation: &mut GitOperation,
        source_commit_oids: &[CommitOid],
        prepared_commit_oids: &[CommitOid],
        raw_summary: String,
        failure_kind: PrepareFailureKind,
        conflict: Option<ConvergenceConflictMetadata>,
    ) -> Result<(), RuntimeError> {
        let summary = format_prepare_failure_summary(&raw_summary);
        let mut updated_workspace = integration_workspace.clone();
        updated_workspace.mark_error(Utc::now());

        let mut updated_convergence = convergence.clone();

        match failure_kind {
            PrepareFailureKind::Conflicted => {
                updated_convergence
                    .transition_to_conflicted(summary.clone(), Utc::now())
                    .map_err(|error| RuntimeError::InvalidState(error.to_string()))?;
            }
            PrepareFailureKind::Failed => {
                updated_convergence.transition_to_failed(Some(summary.clone()), Utc::now());
            }
        }

        let escalation_reason = match failure_kind {
            PrepareFailureKind::Conflicted => EscalationReason::ConvergenceConflict,
            PrepareFailureKind::Failed => EscalationReason::StepFailed,
        };
        let item_escalation = ItemEscalationPatch {
            id: item.id,
            approval_state: match revision.approval_policy {
                ingot_domain::revision::ApprovalPolicy::Required => ApprovalState::NotRequested,
                ingot_domain::revision::ApprovalPolicy::NotRequired => ApprovalState::NotRequired,
            },
            escalation: Escalation::OperatorRequired {
                reason: escalation_reason,
            },
            updated_at: Utc::now(),
        };

        let mut released_queue = queue_entry.clone();
        released_queue.status = ConvergenceQueueEntryStatus::Released;
        released_queue.released_at = Some(Utc::now());
        released_queue.updated_at = Utc::now();

        let mut updated_operation = operation.clone();
        updated_operation.status = GitOperationStatus::Failed;
        updated_operation.completed_at = Some(Utc::now());
        updated_operation
            .payload
            .set_replay_metadata(ConvergenceReplayMetadata {
                source_commit_oids: source_commit_oids.to_vec(),
                prepared_commit_oids: prepared_commit_oids.to_vec(),
                conflict,
            })
            .map_err(|error| RuntimeError::InvalidState(error.to_string()))?;

        let event_type = match failure_kind {
            PrepareFailureKind::Conflicted => ActivityEventType::ConvergenceConflicted,
            PrepareFailureKind::Failed => ActivityEventType::ConvergenceFailed,
        };
        let activities = vec![
            Activity {
                id: ActivityId::new(),
                project_id: project.id,
                event_type,
                subject: ActivitySubject::Convergence(updated_convergence.id),
                payload: serde_json::json!({ "item_id": item.id, "summary": summary }),
                created_at: Utc::now(),
            },
            Activity {
                id: ActivityId::new(),
                project_id: project.id,
                event_type: ActivityEventType::ItemEscalated,
                subject: ActivitySubject::Item(item.id),
                payload: serde_json::json!({ "reason": escalation_reason }),
                created_at: Utc::now(),
            },
        ];
        let event_notifications = activities
            .iter()
            .map(|activity| {
                (
                    activity.project_id,
                    activity.event_type,
                    activity.subject.clone(),
                    activity.payload.clone(),
                )
            })
            .collect::<Vec<_>>();

        PrepareConvergenceFailureRepository::apply_prepare_convergence_failure(
            &self.db,
            PrepareConvergenceFailureMutation {
                workspace: updated_workspace.clone(),
                convergence: updated_convergence.clone(),
                item: item_escalation,
                queue_entry: released_queue,
                git_operation: updated_operation.clone(),
                activities,
            },
        )
        .await?;

        *integration_workspace = updated_workspace;
        *convergence = updated_convergence;
        *operation = updated_operation;

        for (project_id, event_type, subject, payload) in event_notifications {
            self.ui_events
                .publish_entity_changed(project_id, event_type, subject, payload);
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn prepare_queue_head_convergence(
        &self,
        project: &Project,
        item: &ingot_domain::item::Item,
        revision: &ItemRevision,
        jobs: &[Job],
        findings: &[ingot_domain::finding::Finding],
        convergences: &[Convergence],
        queue_entry: &ConvergenceQueueEntry,
    ) -> Result<(), RuntimeError> {
        let _guard = self
            .project_locks
            .acquire_project_mutation(project.id)
            .await;

        let current_item = self.db.get_item(item.id).await?;
        if current_item.current_revision_id != revision.id {
            return Ok(());
        }
        let current_queue = self
            .db
            .find_active_queue_entry_for_revision(revision.id)
            .await?;
        if current_queue
            .as_ref()
            .map(|entry| {
                entry.id != queue_entry.id || entry.status != ConvergenceQueueEntryStatus::Head
            })
            .unwrap_or(true)
        {
            return Ok(());
        }

        if convergences.iter().any(|convergence| {
            convergence.item_revision_id == revision.id && convergence.state.is_active()
        }) {
            return Ok(());
        }

        let source_workspace = self
            .db
            .find_authoring_workspace_for_revision(revision.id)
            .await?
            .ok_or_else(|| RuntimeError::InvalidState("authoring workspace missing".into()))?;
        let source_head_commit_oid = self
            .current_authoring_head_for_revision_with_workspace(revision, jobs)
            .await?
            .ok_or_else(|| RuntimeError::InvalidState("authoring head commit missing".into()))?;
        let paths = self.refresh_project_mirror(project).await?;
        let repo_path = paths.mirror_git_dir.as_path();
        let input_target_commit_oid = resolve_ref_oid(repo_path, &revision.target_ref)
            .await?
            .ok_or_else(|| RuntimeError::InvalidState("target ref unresolved".into()))?;

        let integration_workspace_id = WorkspaceId::new();
        let integration_workspace_path = paths
            .worktree_root
            .join(integration_workspace_id.to_string());
        let integration_workspace_ref =
            GitRef::new(format!("refs/ingot/workspaces/{integration_workspace_id}"));
        let now = Utc::now();
        let mut integration_workspace = Workspace {
            id: integration_workspace_id,
            project_id: project.id,
            kind: WorkspaceKind::Integration,
            strategy: WorkspaceStrategy::Worktree,
            path: integration_workspace_path.clone(),
            created_for_revision_id: Some(revision.id),
            parent_workspace_id: Some(source_workspace.id),
            target_ref: Some(revision.target_ref.clone()),
            workspace_ref: Some(integration_workspace_ref.clone()),
            retention_policy: RetentionPolicy::Persistent,
            created_at: now,
            updated_at: now,
            state: WorkspaceState::Provisioning {
                commits: Some(WorkspaceCommitState::new(
                    input_target_commit_oid.clone(),
                    input_target_commit_oid.clone(),
                )),
            },
        };
        self.db.create_workspace(&integration_workspace).await?;

        let provisioned = provision_integration_workspace(
            repo_path,
            &integration_workspace_path,
            &integration_workspace_ref,
            &input_target_commit_oid,
        )
        .await?;
        integration_workspace.path = provisioned.workspace_path.clone();
        integration_workspace.workspace_ref = Some(provisioned.workspace_ref);
        integration_workspace.set_head_commit_oid(provisioned.head_commit_oid, Utc::now());
        self.db.update_workspace(&integration_workspace).await?;

        let mut convergence = Convergence {
            id: ingot_domain::ids::ConvergenceId::new(),
            project_id: project.id,
            item_id: item.id,
            item_revision_id: revision.id,
            source_workspace_id: source_workspace.id,
            source_head_commit_oid: source_head_commit_oid.clone(),
            target_ref: revision.target_ref.clone(),
            strategy: ingot_domain::convergence::ConvergenceStrategy::RebaseThenFastForward,
            target_head_valid: Some(true),
            created_at: now,
            state: ingot_domain::convergence::ConvergenceState::Running {
                integration_workspace_id: integration_workspace.id,
                input_target_commit_oid: input_target_commit_oid.clone(),
            },
        };
        self.db.create_convergence(&convergence).await?;
        self.append_activity(
            project.id,
            ActivityEventType::ConvergenceStarted,
            ActivitySubject::Convergence(convergence.id),
            serde_json::json!({ "item_id": item.id, "queue_entry_id": queue_entry.id }),
        )
        .await?;

        let source_base_commit_oid = self
            .effective_authoring_base_commit_oid(revision)
            .await?
            .ok_or_else(|| RuntimeError::InvalidState("authoring base commit missing".into()))?;
        let source_commit_oids =
            list_commits_oldest_first(repo_path, &source_base_commit_oid, &source_head_commit_oid)
                .await?;
        let mut operation = GitOperation {
            id: GitOperationId::new(),
            project_id: project.id,
            entity: GitOperationEntityRef::Convergence(convergence.id),
            payload: OperationPayload::PrepareConvergenceCommit {
                workspace_id: integration_workspace.id,
                ref_name: integration_workspace.workspace_ref.clone(),
                expected_old_oid: input_target_commit_oid.clone(),
                commit_oid: None,
                replay_metadata: Some(ConvergenceReplayMetadata {
                    source_commit_oids: source_commit_oids.clone(),
                    prepared_commit_oids: vec![],
                    conflict: None,
                }),
            },
            status: GitOperationStatus::Planned,
            created_at: now,
            completed_at: None,
        };
        self.db.create_git_operation(&operation).await?;
        self.append_activity(
            project.id,
            ActivityEventType::GitOperationPlanned,
            ActivitySubject::GitOperation(operation.id),
            serde_json::json!({ "operation_kind": operation.operation_kind(), "entity_id": operation.entity.entity_id_string() }),
        )
        .await?;

        let integration_workspace_dir = PathBuf::from(&integration_workspace.path);
        let mut prepared_tip = input_target_commit_oid.clone();
        let mut prepared_commit_oids = Vec::with_capacity(source_commit_oids.len());

        for source_commit_oid in &source_commit_oids {
            if let Err(error) =
                cherry_pick_no_commit(&integration_workspace_dir, source_commit_oid).await
            {
                let git_error = truncate_convergence_conflict_git_error(&error.to_string());
                let (conflict_files, total_conflict_file_count) =
                    match collect_convergence_conflict_files(&integration_workspace_dir).await {
                        Ok(collected) => (collected.files, collected.total_count),
                        Err(error) => {
                            warn!(
                                ?error,
                                convergence_id = %convergence.id,
                                source_commit_oid = %source_commit_oid,
                                "failed to collect convergence conflict files"
                            );
                            (Vec::new(), 0)
                        }
                    };
                let summary = format_convergence_conflict_summary(
                    source_commit_oid,
                    &conflict_files,
                    total_conflict_file_count,
                    &git_error,
                );
                let files_truncated = total_conflict_file_count > conflict_files.len();
                let conflict = ConvergenceConflictMetadata {
                    failed_source_commit_oid: source_commit_oid.clone(),
                    git_error,
                    total_file_count: total_conflict_file_count,
                    files_truncated,
                    files: conflict_files,
                };
                if let Err(error) = abort_cherry_pick(&integration_workspace_dir).await {
                    warn!(
                        ?error,
                        convergence_id = %convergence.id,
                        source_commit_oid = %source_commit_oid,
                        "failed to abort conflicted convergence cherry-pick"
                    );
                }
                self.fail_prepare_convergence_attempt(
                    project,
                    item,
                    revision,
                    queue_entry,
                    &mut integration_workspace,
                    &mut convergence,
                    &mut operation,
                    &source_commit_oids,
                    &prepared_commit_oids,
                    summary,
                    PrepareFailureKind::Conflicted,
                    Some(conflict),
                )
                .await?;
                return Ok(());
            }

            let has_replay_changes =
                match working_tree_has_changes(&integration_workspace_dir).await {
                    Ok(has_changes) => has_changes,
                    Err(error) => {
                        self.fail_prepare_convergence_attempt(
                            project,
                            item,
                            revision,
                            queue_entry,
                            &mut integration_workspace,
                            &mut convergence,
                            &mut operation,
                            &source_commit_oids,
                            &prepared_commit_oids,
                            error.to_string(),
                            PrepareFailureKind::Failed,
                            None,
                        )
                        .await?;
                        return Ok(());
                    }
                };
            if !has_replay_changes {
                continue;
            }

            let original_message = match commit_message(repo_path, source_commit_oid).await {
                Ok(message) => message,
                Err(error) => {
                    self.fail_prepare_convergence_attempt(
                        project,
                        item,
                        revision,
                        queue_entry,
                        &mut integration_workspace,
                        &mut convergence,
                        &mut operation,
                        &source_commit_oids,
                        &prepared_commit_oids,
                        error.to_string(),
                        PrepareFailureKind::Failed,
                        None,
                    )
                    .await?;
                    return Ok(());
                }
            };
            let next_prepared_tip = match ingot_git::commit::create_daemon_convergence_commit(
                &integration_workspace_dir,
                &original_message,
                &ConvergenceCommitTrailers {
                    operation_id: operation.id,
                    item_id: item.id,
                    revision_no: revision.revision_no,
                    convergence_id: convergence.id,
                    source_commit_oid: source_commit_oid.clone(),
                },
            )
            .await
            {
                Ok(prepared_tip) => prepared_tip,
                Err(error) => {
                    self.fail_prepare_convergence_attempt(
                        project,
                        item,
                        revision,
                        queue_entry,
                        &mut integration_workspace,
                        &mut convergence,
                        &mut operation,
                        &source_commit_oids,
                        &prepared_commit_oids,
                        error.to_string(),
                        PrepareFailureKind::Failed,
                        None,
                    )
                    .await?;
                    return Ok(());
                }
            };
            if let Some(workspace_ref) = integration_workspace.workspace_ref.as_ref() {
                if let Err(error) = git(
                    repo_path,
                    &[
                        "update-ref",
                        workspace_ref.as_str(),
                        next_prepared_tip.as_str(),
                    ],
                )
                .await
                {
                    self.fail_prepare_convergence_attempt(
                        project,
                        item,
                        revision,
                        queue_entry,
                        &mut integration_workspace,
                        &mut convergence,
                        &mut operation,
                        &source_commit_oids,
                        &prepared_commit_oids,
                        error.to_string(),
                        PrepareFailureKind::Failed,
                        None,
                    )
                    .await?;
                    return Ok(());
                }
            }
            prepared_tip = next_prepared_tip;
            prepared_commit_oids.push(prepared_tip.clone());
        }

        integration_workspace.mark_ready_with_head(prepared_tip.clone(), Utc::now());
        self.db.update_workspace(&integration_workspace).await?;

        convergence
            .transition_to_prepared(prepared_tip.clone(), Some(Utc::now()))
            .map_err(|error| RuntimeError::InvalidState(error.to_string()))?;
        self.db.update_convergence(&convergence).await?;

        operation
            .payload
            .set_convergence_commit_result(prepared_tip.clone())
            .map_err(|error| RuntimeError::InvalidState(error.to_string()))?;
        operation
            .payload
            .set_replay_metadata(ConvergenceReplayMetadata {
                source_commit_oids,
                prepared_commit_oids,
                conflict: None,
            })
            .map_err(|error| RuntimeError::InvalidState(error.to_string()))?;
        self.mark_git_operation_reconciled(&mut operation).await?;

        let mut all_convergences = convergences.to_vec();
        all_convergences.push(convergence.clone());
        let validation_job = dispatch_job(
            &current_item,
            revision,
            jobs,
            findings,
            &all_convergences,
            DispatchJobCommand {
                step_id: Some(StepId::ValidateIntegrated),
            },
        )
        .map_err(|error| RuntimeError::InvalidState(error.to_string()))?;
        self.db.create_job(&validation_job).await?;
        self.append_activity(
            project.id,
            ActivityEventType::ConvergencePrepared,
            ActivitySubject::Convergence(convergence.id),
            serde_json::json!({ "item_id": item.id, "validation_job_id": validation_job.id }),
        )
        .await?;
        self.append_activity(
            project.id,
            ActivityEventType::JobDispatched,
            ActivitySubject::Job(validation_job.id),
            serde_json::json!({ "item_id": item.id, "step_id": validation_job.step_id }),
        )
        .await?;

        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) async fn reconcile_checkout_sync_state(
        &self,
        project: &Project,
        item_id: ingot_domain::ids::ItemId,
        revision: &ItemRevision,
        prepared_commit_oid: Option<&CommitOid>,
    ) -> Result<CheckoutSyncStatus, RuntimeError> {
        let mut item = self.db.get_item(item_id).await?;
        let status = match prepared_commit_oid {
            Some(prepared_commit_oid) => {
                let paths = self.refresh_project_mirror(project).await?;
                ingot_git::project_repo::checkout_sync_status_for_commit(
                    &project.path,
                    paths.mirror_git_dir.as_path(),
                    &revision.target_ref,
                    prepared_commit_oid,
                )
                .await?
            }
            None => checkout_sync_status(&project.path, &revision.target_ref).await?,
        };
        let checkout_sync_blocked = matches!(
            item.escalation,
            Escalation::OperatorRequired {
                reason: EscalationReason::CheckoutSyncBlocked
            }
        );
        match &status {
            CheckoutSyncStatus::Ready => {
                if checkout_sync_blocked {
                    item.escalation = Escalation::None;
                    item.updated_at = Utc::now();
                    self.db.update_item(&item).await?;
                    self.append_activity(
                        project.id,
                        ActivityEventType::CheckoutSyncCleared,
                        ActivitySubject::Item(item.id),
                        serde_json::json!({}),
                    )
                    .await?;
                    self.append_activity(
                        project.id,
                        ActivityEventType::ItemEscalationCleared,
                        ActivitySubject::Item(item.id),
                        serde_json::json!({ "reason": "checkout_sync_ready" }),
                    )
                    .await?;
                }
            }
            CheckoutSyncStatus::Blocked { message, .. } => {
                if !checkout_sync_blocked && !item.lifecycle.is_done() {
                    item.escalation = Escalation::OperatorRequired {
                        reason: EscalationReason::CheckoutSyncBlocked,
                    };
                    item.updated_at = Utc::now();
                    self.db.update_item(&item).await?;
                    self.append_activity(
                        project.id,
                        ActivityEventType::CheckoutSyncBlocked,
                        ActivitySubject::Item(item.id),
                        serde_json::json!({ "message": message }),
                    )
                    .await?;
                    self.append_activity(
                        project.id,
                        ActivityEventType::ItemEscalated,
                        ActivitySubject::Item(item.id),
                        serde_json::json!({ "reason": EscalationReason::CheckoutSyncBlocked }),
                    )
                    .await?;
                }
            }
        }

        Ok(status)
    }
}

fn format_convergence_conflict_summary(
    failed_source_commit_oid: &CommitOid,
    files: &[ConvergenceConflictFile],
    total_file_count: usize,
    git_error: &str,
) -> String {
    let short_oid = failed_source_commit_oid
        .as_str()
        .chars()
        .take(12)
        .collect::<String>();
    let first_error_line = git_error
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("git cherry-pick failed");
    if files.is_empty() {
        return format!("Cherry-pick conflict while replaying {short_oid}: {first_error_line}");
    }

    // Keep the persisted human summary compact; structured metadata carries the larger file list.
    let displayed_paths = files.iter().take(5).collect::<Vec<_>>();
    let display_paths = displayed_paths
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let remaining = total_file_count.saturating_sub(displayed_paths.len());
    let suffix = if remaining == 0 {
        String::new()
    } else {
        format!(" (+{remaining} more)")
    };

    format!(
        "Cherry-pick conflict while replaying {short_oid} in {total_file_count} {}: {display_paths}{suffix}; {first_error_line}",
        if total_file_count == 1 {
            "file"
        } else {
            "files"
        }
    )
}

fn format_prepare_failure_summary(raw_summary: &str) -> String {
    let sanitized = sanitize_prepare_failure_summary(raw_summary);
    let summary = sanitized.trim();
    if summary.is_empty() {
        return "prepare convergence failed".to_owned();
    }

    // Human summaries stay marker-free for table/activity display; raw git_error metadata carries
    // the explicit "[truncated]" marker where operators can inspect the persisted diagnostic text.
    truncate_summary_to_char_boundary(summary, MAX_PREPARE_FAILURE_SUMMARY_BYTES)
}

fn sanitize_prepare_failure_summary(value: &str) -> String {
    value
        .chars()
        .filter(|ch| matches!(ch, '\n' | '\r' | '\t') || !ch.is_control())
        .collect()
}

fn truncate_summary_to_char_boundary(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }

    let end = value
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= max_bytes)
        .last()
        .unwrap_or(0);
    value[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_prepare_failure_summary_caps_without_metadata_marker() {
        let raw_summary = format!("first line\n{}\nlast line", "é".repeat(2_000));

        let summary = format_prepare_failure_summary(&raw_summary);

        assert!(summary.len() <= 2 * 1024);
        assert!(summary.is_char_boundary(summary.len()));
        assert!(summary.starts_with("first line\n"));
        assert!(!summary.ends_with("\n[truncated]"));
    }

    #[test]
    fn format_prepare_failure_summary_strips_control_bytes() {
        let summary = format_prepare_failure_summary("\u{1b}[31mfailed\u{0}\nnext");

        assert_eq!(summary, "[31mfailed\nnext");
    }

    #[test]
    fn format_convergence_conflict_summary_pluralizes_file_count() {
        let file = ConvergenceConflictFile {
            path: "tracked.txt".into(),
            stages: Vec::new(),
            excerpt: None,
        };

        let one = format_convergence_conflict_summary(
            &CommitOid::new("0123456789abcdef"),
            std::slice::from_ref(&file),
            1,
            "git failed",
        );
        let two = format_convergence_conflict_summary(
            &CommitOid::new("0123456789abcdef"),
            &[file],
            2,
            "git failed",
        );

        assert!(one.contains("in 1 file:"));
        assert!(two.contains("in 2 files:"));
        assert!(two.contains("(+1 more)"));
    }

    #[test]
    fn format_convergence_conflict_summary_uses_default_for_blank_git_error() {
        let file = ConvergenceConflictFile {
            path: "tracked.txt".into(),
            stages: Vec::new(),
            excerpt: None,
        };

        let summary = format_convergence_conflict_summary(
            &CommitOid::new("0123456789abcdef"),
            &[file],
            1,
            "\n",
        );

        assert!(summary.ends_with("git cherry-pick failed"));
    }

    #[test]
    fn format_convergence_conflict_summary_handles_missing_file_metadata() {
        let summary = format_convergence_conflict_summary(
            &CommitOid::new("0123456789abcdef"),
            &[],
            2,
            "git failed",
        );

        assert_eq!(
            summary,
            "Cherry-pick conflict while replaying 0123456789ab: git failed"
        );
    }
}
