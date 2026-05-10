mod finalization {
    pub use super::super::flow::{
        finalize_prepared_convergence, should_auto_finalize_prepared_convergence,
        should_invalidate_prepared_convergence, should_prepare_convergence,
    };
}

mod types {
    pub use super::super::flow::{
        ApprovalFinalizeReadiness, ConvergenceApprovalContext, ConvergenceCommandPort,
        ConvergenceSystemActionPort, FinalizePreparedTrigger, PreparedConvergenceFinalizePort,
        RejectApprovalTeardown,
    };
}

mod command {
    use chrono::{DateTime, Utc};
    use ingot_domain::activity::{Activity, ActivityEventType, ActivitySubject};
    use ingot_domain::convergence_queue::{ConvergenceQueueEntry, ConvergenceQueueEntryStatus};
    use ingot_domain::ids::{ActivityId, ItemId, ProjectId};
    use ingot_domain::item::ApprovalState;
    use ingot_domain::revision::ItemRevision;

    use crate::UseCaseError;
    use crate::item::approval_state_for_policy;

    use super::finalization::{finalize_prepared_convergence, should_prepare_convergence};
    use super::types::{
        ApprovalFinalizeReadiness, ConvergenceApprovalContext, ConvergenceCommandPort,
        FinalizePreparedTrigger, PreparedConvergenceFinalizePort, RejectApprovalTeardown,
    };

    #[derive(Clone)]
    pub struct ConvergenceService<P> {
        pub(super) port: P,
    }

    impl<P> ConvergenceService<P> {
        pub fn new(port: P) -> Self {
            Self { port }
        }
    }

    #[must_use]
    pub fn build_convergence_queue_entry(
        project_id: ProjectId,
        item_id: ItemId,
        revision: &ItemRevision,
        lane_head_exists: bool,
        now: DateTime<Utc>,
    ) -> ConvergenceQueueEntry {
        ConvergenceQueueEntry {
            id: ingot_domain::ids::ConvergenceQueueEntryId::new(),
            project_id,
            item_id,
            item_revision_id: revision.id,
            target_ref: revision.target_ref.clone(),
            status: if lane_head_exists {
                ConvergenceQueueEntryStatus::Queued
            } else {
                ConvergenceQueueEntryStatus::Head
            },
            head_acquired_at: (!lane_head_exists).then_some(now),
            created_at: now,
            updated_at: now,
            released_at: None,
        }
    }

    impl<P> ConvergenceService<P>
    where
        P: ConvergenceCommandPort + PreparedConvergenceFinalizePort,
    {
        pub async fn queue_prepare(
            &self,
            project_id: ProjectId,
            item_id: ItemId,
        ) -> Result<(), UseCaseError> {
            let context = self
                .port
                .load_queue_prepare_context(project_id, item_id)
                .await?;
            if context.item.project_id != project_id {
                return Err(UseCaseError::ItemNotFound);
            }

            if context.active_queue_entry.is_none()
                && !should_prepare_convergence(
                    &context.item,
                    &context.revision,
                    &context.jobs,
                    &context.findings,
                    &context.convergences,
                )
            {
                return Err(UseCaseError::ConvergenceNotPreparable);
            }

            let mut queue_entry = if let Some(queue_entry) = context.active_queue_entry {
                queue_entry
            } else {
                let now = Utc::now();
                let queue_entry = build_convergence_queue_entry(
                    context.project.id,
                    context.item.id,
                    &context.revision,
                    context.lane_head.is_some(),
                    now,
                );
                self.port.create_queue_entry(&queue_entry).await?;
                self.port
                    .append_activity(&Activity {
                        id: ActivityId::new(),
                        project_id: context.project.id,
                        event_type: ActivityEventType::ConvergenceQueued,
                        subject: ActivitySubject::QueueEntry(queue_entry.id),
                        payload: serde_json::json!({
                            "item_id": context.item.id,
                            "target_ref": context.revision.target_ref,
                        }),
                        created_at: now,
                    })
                    .await?;
                queue_entry
            };

            if queue_entry.status == ConvergenceQueueEntryStatus::Queued
                && context.lane_head.is_none()
            {
                queue_entry.status = ConvergenceQueueEntryStatus::Head;
                queue_entry.head_acquired_at = Some(Utc::now());
                queue_entry.updated_at = Utc::now();
                self.port.update_queue_entry(&queue_entry).await?;
                self.port
                    .append_activity(&Activity {
                        id: ActivityId::new(),
                        project_id: context.project.id,
                        event_type: ActivityEventType::ConvergenceLaneAcquired,
                        subject: ActivitySubject::QueueEntry(queue_entry.id),
                        payload: serde_json::json!({
                            "item_id": context.item.id,
                            "target_ref": context.revision.target_ref,
                        }),
                        created_at: Utc::now(),
                    })
                    .await?;
            }

            Ok(())
        }

        pub async fn approve_item(
            &self,
            project_id: ProjectId,
            item_id: ItemId,
        ) -> Result<(), UseCaseError> {
            let ConvergenceApprovalContext {
                project,
                item,
                revision,
                has_active_job,
                has_active_convergence,
                finalize_readiness,
            } = self.port.load_approval_context(project_id, item_id).await?;

            if item.approval_state != ApprovalState::Pending {
                return Err(UseCaseError::ApprovalNotPending);
            }
            if has_active_job {
                return Err(UseCaseError::ActiveJobExists);
            }
            if has_active_convergence {
                return Err(UseCaseError::ActiveConvergenceExists);
            }
            let (convergence, queue_entry) = match finalize_readiness {
                ApprovalFinalizeReadiness::MissingPreparedConvergence => {
                    return Err(UseCaseError::PreparedConvergenceMissing);
                }
                ApprovalFinalizeReadiness::PreparedConvergenceStale => {
                    return Err(UseCaseError::PreparedConvergenceStale);
                }
                ApprovalFinalizeReadiness::ConvergenceNotQueued => {
                    return Err(UseCaseError::ConvergenceNotQueued);
                }
                ApprovalFinalizeReadiness::ConvergenceNotLaneHead => {
                    return Err(UseCaseError::ConvergenceNotLaneHead);
                }
                ApprovalFinalizeReadiness::Ready {
                    convergence,
                    queue_entry,
                } => (convergence, queue_entry),
            };

            finalize_prepared_convergence(
                &self.port,
                FinalizePreparedTrigger::ApprovalCommand,
                &project,
                &item,
                &revision,
                &convergence,
                &queue_entry,
            )
            .await?;

            self.port
                .append_activity(&Activity {
                    id: ActivityId::new(),
                    project_id,
                    event_type: ActivityEventType::ApprovalApproved,
                    subject: ActivitySubject::Item(item.id),
                    payload: serde_json::json!({
                        "convergence_id": convergence.id,
                        "queue_entry_id": queue_entry.id,
                    }),
                    created_at: Utc::now(),
                })
                .await?;
            Ok(())
        }

        pub async fn reject_item_approval(
            &self,
            project_id: ProjectId,
            item_id: ItemId,
            next_revision: &ItemRevision,
        ) -> Result<RejectApprovalTeardown, UseCaseError> {
            let mut context = self
                .port
                .load_reject_approval_context(project_id, item_id)
                .await?;
            if context.item.approval_state != ApprovalState::Pending {
                return Err(UseCaseError::ApprovalNotPending);
            }
            if context.has_active_job {
                return Err(UseCaseError::ActiveJobExists);
            }
            if context.has_active_convergence {
                return Err(UseCaseError::ActiveConvergenceExists);
            }
            let teardown = self
                .port
                .teardown_reject_approval(project_id, item_id)
                .await?;
            if !teardown.has_cancelled_convergence {
                return Err(UseCaseError::PreparedConvergenceMissing);
            }

            context.item.current_revision_id = next_revision.id;
            context.item.approval_state = approval_state_for_policy(next_revision.approval_policy);
            context.item.escalation = ingot_domain::item::Escalation::None;
            context.item.updated_at = Utc::now();
            self.port
                .apply_rejected_approval(&context.item, next_revision)
                .await?;
            Ok(teardown)
        }
    }
}

pub use command::{ConvergenceService, build_convergence_queue_entry};

mod system_actions {
    use chrono::Utc;
    use ingot_domain::activity::{Activity, ActivityEventType, ActivitySubject};
    use ingot_domain::convergence::{Convergence, ConvergenceStatus};
    use ingot_domain::convergence_queue::ConvergenceQueueEntryStatus;
    use ingot_domain::item::Item;
    use ingot_domain::ports::InvalidatePreparedConvergenceMutation;
    use ingot_domain::revision::ItemRevision;

    use crate::UseCaseError;
    use crate::item::approval_state_for_policy;
    use crate::store::{ConvergenceQueuePromotionStore, PreparedConvergenceInvalidationStore};

    use super::command::ConvergenceService;
    use super::finalization::{
        should_auto_finalize_prepared_convergence, should_invalidate_prepared_convergence,
        should_prepare_convergence,
    };
    use super::types::ConvergenceSystemActionPort;

    impl<P> ConvergenceService<P>
    where
        P: ConvergenceSystemActionPort,
    {
        pub async fn tick_system_actions(&self) -> Result<bool, UseCaseError> {
            let projects = self.port.load_system_action_projects().await?;

            for project_state in projects {
                self.port
                    .promote_queue_heads(project_state.project.id)
                    .await?;

                for state in &project_state.items {
                    if should_invalidate_prepared_convergence(
                        &state.item,
                        &state.revision,
                        &state.jobs,
                        &state.findings,
                        &state.convergences,
                    ) {
                        self.port
                            .invalidate_prepared_convergence(
                                project_state.project.id,
                                state.item_id,
                            )
                            .await?;
                        return Ok(true);
                    }

                    let has_prepared_convergence = state.convergences.iter().any(|convergence| {
                        convergence.item_revision_id == state.revision.id
                            && convergence.state.status() == ConvergenceStatus::Prepared
                    });

                    if let Some(queue_entry) = state.queue_entry.as_ref() {
                        let should_prepare_queue_head = queue_entry.status
                            == ConvergenceQueueEntryStatus::Head
                            && should_prepare_convergence(
                                &state.item,
                                &state.revision,
                                &state.jobs,
                                &state.findings,
                                &state.convergences,
                            );

                        if should_prepare_queue_head {
                            self.port
                                .prepare_queue_head_convergence(
                                    &project_state.project,
                                    state,
                                    queue_entry,
                                )
                                .await?;
                            return Ok(true);
                        }

                        let should_finalize = has_prepared_convergence
                            && should_auto_finalize_prepared_convergence(
                                &state.item,
                                &state.revision,
                                &state.jobs,
                                &state.findings,
                                &state.convergences,
                                Some(queue_entry),
                            );

                        if should_finalize
                            && self
                                .port
                                .auto_finalize_prepared_convergence(
                                    project_state.project.id,
                                    state.item_id,
                                )
                                .await?
                        {
                            return Ok(true);
                        }
                    } else if project_state.project.execution_mode
                        == ingot_domain::project::ExecutionMode::Autopilot
                        && should_prepare_convergence(
                            &state.item,
                            &state.revision,
                            &state.jobs,
                            &state.findings,
                            &state.convergences,
                        )
                        && self
                            .port
                            .auto_queue_convergence(project_state.project.id, state.item_id)
                            .await?
                    {
                        return Ok(true);
                    }
                }
            }

            Ok(false)
        }
    }

    pub async fn promote_queue_heads<S>(
        store: &S,
        project_id: ingot_domain::ids::ProjectId,
    ) -> Result<bool, UseCaseError>
    where
        S: ConvergenceQueuePromotionStore,
    {
        let entries = store.list_active_by_project(project_id).await?;
        let mut lanes_with_heads = entries
            .iter()
            .filter(|entry| entry.status == ConvergenceQueueEntryStatus::Head)
            .map(|entry| entry.target_ref.clone())
            .collect::<std::collections::HashSet<_>>();

        let mut promoted = false;
        for entry in entries {
            if entry.status != ConvergenceQueueEntryStatus::Queued
                || lanes_with_heads.contains(&entry.target_ref)
            {
                continue;
            }

            let mut entry = entry;
            entry.status = ConvergenceQueueEntryStatus::Head;
            entry.head_acquired_at = Some(Utc::now());
            entry.updated_at = Utc::now();
            store.update(&entry).await?;
            store
                .append(&Activity {
                    id: ingot_domain::ids::ActivityId::new(),
                    project_id,
                    event_type: ActivityEventType::ConvergenceLaneAcquired,
                    subject: ActivitySubject::QueueEntry(entry.id),
                    payload: serde_json::json!({
                        "item_id": entry.item_id,
                        "target_ref": entry.target_ref,
                    }),
                    created_at: Utc::now(),
                })
                .await?;
            lanes_with_heads.insert(entry.target_ref);
            promoted = true;
        }

        Ok(promoted)
    }

    pub async fn invalidate_prepared_convergence<S>(
        store: &S,
        item: &mut Item,
        revision: &ItemRevision,
        convergences: &[Convergence],
    ) -> Result<bool, UseCaseError>
    where
        S: PreparedConvergenceInvalidationStore,
    {
        let mut convergence = match convergences
            .iter()
            .find(|convergence| {
                convergence.item_revision_id == revision.id
                    && convergence.state.status() == ConvergenceStatus::Prepared
            })
            .cloned()
        {
            Some(c) => c,
            None => return Ok(false),
        };

        convergence.transition_to_failed(Some("target_ref_moved".into()), Utc::now());

        let workspace_update =
            if let Some(workspace_id) = convergence.state.integration_workspace_id() {
                let mut workspace = store.get(workspace_id).await?;
                workspace.mark_stale(Utc::now());
                Some(workspace)
            } else {
                None
            };

        item.approval_state = approval_state_for_policy(revision.approval_policy);
        item.updated_at = Utc::now();

        let activity = Activity {
            id: ingot_domain::ids::ActivityId::new(),
            project_id: convergence.project_id,
            event_type: ActivityEventType::ConvergenceFailed,
            subject: ActivitySubject::Convergence(convergence.id),
            payload: serde_json::json!({ "item_id": item.id, "reason": "target_ref_moved" }),
            created_at: Utc::now(),
        };

        store
            .apply_invalidate_prepared_convergence(InvalidatePreparedConvergenceMutation {
                convergence,
                workspace_update,
                item: item.clone(),
                activity,
            })
            .await?;

        Ok(true)
    }
}

pub use system_actions::{invalidate_prepared_convergence, promote_queue_heads};
