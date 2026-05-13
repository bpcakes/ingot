use chrono::Utc;
use ingot_domain::ids::{ItemId, ItemRevisionId, ProjectId};
use ingot_domain::test_support::{
    ConvergenceBuilder, ItemBuilder, ProjectBuilder, RevisionBuilder,
};
use ingot_test_support::git::unique_temp_path;
use uuid::Uuid;

use crate::UseCaseError;

use super::test_support::{FakePort, project_state};
use super::{
    ConvergenceService, FinalizePreparedTrigger, FinalizeTargetRefResult,
    finalize_prepared_convergence,
};

fn prepared_finalization_parts() -> (
    ingot_domain::project::Project,
    ingot_domain::item::Item,
    ingot_domain::revision::ItemRevision,
    ingot_domain::convergence::Convergence,
    ingot_domain::convergence_queue::ConvergenceQueueEntry,
) {
    let now = Utc::now();
    let project_id = ProjectId::from_uuid(Uuid::nil());
    let item_id = ItemId::from_uuid(Uuid::nil());
    let revision_id = ItemRevisionId::from_uuid(Uuid::nil());
    let project = ProjectBuilder::new(unique_temp_path("ingot-convergence"))
        .id(project_id)
        .created_at(now)
        .build();
    let item = ItemBuilder::new(project_id, revision_id)
        .id(item_id)
        .created_at(now)
        .build();
    let revision = RevisionBuilder::new(item_id)
        .id(revision_id)
        .explicit_seed("abc123")
        .created_at(now)
        .build();
    let convergence = ConvergenceBuilder::new(project_id, item_id, revision_id)
        .id(ingot_domain::ids::ConvergenceId::from_uuid(Uuid::nil()))
        .status(ingot_domain::convergence::ConvergenceStatus::Prepared)
        .target_head_valid(true)
        .created_at(now)
        .build();
    let queue_entry = ingot_domain::convergence_queue::ConvergenceQueueEntry {
        id: ingot_domain::ids::ConvergenceQueueEntryId::from_uuid(Uuid::nil()),
        project_id,
        item_id,
        item_revision_id: revision_id,
        target_ref: "refs/heads/main".into(),
        status: ingot_domain::convergence_queue::ConvergenceQueueEntryStatus::Head,
        head_acquired_at: Some(now),
        created_at: now,
        updated_at: now,
        released_at: None,
    };

    (project, item, revision, convergence, queue_entry)
}

async fn finalize_with(port: &FakePort) -> Result<(), UseCaseError> {
    let (project, item, revision, convergence, queue_entry) = prepared_finalization_parts();
    finalize_prepared_convergence(
        port,
        FinalizePreparedTrigger::ApprovalCommand,
        &project,
        &item,
        &revision,
        &convergence,
        &queue_entry,
    )
    .await
}

#[tokio::test]
async fn invalidation_wins_first() {
    let port = FakePort::with_projects(vec![project_state("invalidate_prepared_convergence")]);
    let service = ConvergenceService::new(port.clone());

    let made_progress = service
        .tick_system_actions()
        .await
        .expect("tick system actions");

    assert!(made_progress);
    assert!(
        port.calls()
            .iter()
            .any(|call| call.starts_with("invalidate:"))
    );
}

#[tokio::test]
async fn prepare_runs_for_queue_head() {
    let port = FakePort::with_projects(vec![project_state("prepare_convergence")]);
    let service = ConvergenceService::new(port.clone());

    let made_progress = service
        .tick_system_actions()
        .await
        .expect("tick system actions");

    assert!(made_progress);
    assert!(port.calls().iter().any(|call| call.starts_with("prepare:")));
}

#[tokio::test]
async fn blocked_auto_finalize_does_not_count_as_progress() {
    let port = FakePort {
        auto_finalize_progress: false,
        ..FakePort::with_projects(vec![project_state("finalize_prepared_convergence")])
    };
    let service = ConvergenceService::new(port.clone());

    let made_progress = service
        .tick_system_actions()
        .await
        .expect("tick system actions");

    assert!(!made_progress);
    let calls = port.calls();
    assert!(calls.iter().any(|call| call.starts_with("finalize:")));
    assert!(!calls.iter().any(|call| call.starts_with("prepare:")));
}

#[tokio::test]
async fn blocked_auto_finalize_allows_later_system_action_to_run() {
    let port = FakePort {
        auto_finalize_progress: false,
        ..FakePort::with_projects(vec![
            project_state("finalize_prepared_convergence"),
            project_state("prepare_convergence"),
        ])
    };
    let service = ConvergenceService::new(port.clone());

    let made_progress = service
        .tick_system_actions()
        .await
        .expect("tick system actions");

    assert!(made_progress);
    let calls = port.calls();
    let finalize_index = calls
        .iter()
        .position(|call| call.starts_with("finalize:"))
        .expect("finalize call");
    let prepare_index = calls
        .iter()
        .position(|call| call.starts_with("prepare:"))
        .expect("prepare call");
    assert!(finalize_index < prepare_index);
}

#[tokio::test]
async fn approve_item_uses_shared_finalizer_for_already_finalized_target() {
    let port = FakePort {
        finalize_target_ref_result: FinalizeTargetRefResult::AlreadyFinalized,
        ..FakePort::with_projects(Vec::new())
    };

    finalize_with(&port)
        .await
        .expect("finalization should complete");

    let calls = port.calls();
    assert!(
        calls
            .iter()
            .any(|call| call == "update_op:Applied" || call == "update_op:Reconciled")
    );
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("finalize_target_ref:"))
    );
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("apply_finalization_mutation:target_ref_advanced:"))
    );
    assert!(calls.iter().any(|call| {
        call.starts_with("apply_finalization_mutation:checkout_adoption_succeeded:")
    }));
}

#[tokio::test]
async fn approve_item_leaves_finalize_unresolved_when_checkout_sync_stays_blocked() {
    let port = FakePort {
        checkout_finalization_readiness: super::CheckoutFinalizationReadiness::Blocked {
            message: "registered checkout blocked".into(),
        },
        ..FakePort::with_projects(Vec::new())
    };

    finalize_with(&port)
        .await
        .expect("finalization should advance target even when checkout sync stays blocked");

    let calls = port.calls();
    assert!(calls.iter().any(|call| call == "update_op:Applied"));
    assert!(!calls.iter().any(|call| call == "update_op:Reconciled"));
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("apply_finalization_mutation:target_ref_advanced:"))
    );
    assert!(
        !calls.iter().any(
            |call| call.starts_with("apply_finalization_mutation:checkout_adoption_succeeded:")
        )
    );
    assert!(!calls.iter().any(|call| call.starts_with("sync_checkout:")));
}

#[tokio::test]
async fn approve_item_keeps_finalize_operation_unresolved_when_sync_retry_fails() {
    let port = FakePort {
        checkout_finalization_readiness: super::CheckoutFinalizationReadiness::NeedsSync,
        sync_checkout_should_fail: true,
        ..FakePort::with_projects(Vec::new())
    };

    finalize_with(&port)
        .await
        .expect("finalization should keep operation unresolved when sync retry fails");

    let calls = port.calls();
    assert!(calls.iter().any(|call| call == "update_op:Applied"));
    assert!(!calls.iter().any(|call| call == "update_op:Reconciled"));
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("apply_finalization_mutation:target_ref_advanced:"))
    );
    assert!(
        !calls.iter().any(
            |call| call.starts_with("apply_finalization_mutation:checkout_adoption_succeeded:")
        )
    );
    assert!(calls.iter().any(|call| call.starts_with("sync_checkout:")));
}

#[tokio::test]
async fn approve_item_surfaces_checkout_readiness_failures_after_finalize() {
    let port = FakePort {
        checkout_finalization_readiness_error: Some("git inspection failed".into()),
        ..FakePort::with_projects(Vec::new())
    };

    let error = finalize_with(&port)
        .await
        .expect_err("approval should surface checkout readiness failures");

    assert!(matches!(error, UseCaseError::Internal(message) if message == "git inspection failed"));
    let calls = port.calls();
    assert!(calls.iter().any(|call| call == "update_op:Applied"));
    assert!(!calls.iter().any(|call| call == "update_op:Reconciled"));
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("apply_finalization_mutation:target_ref_advanced:"))
    );
    assert!(
        !calls.iter().any(
            |call| call.starts_with("apply_finalization_mutation:checkout_adoption_succeeded:")
        )
    );
    assert!(!calls.iter().any(|call| call.starts_with("sync_checkout:")));
}

#[tokio::test]
async fn approve_item_keeps_finalize_operation_unresolved_when_target_ref_advance_persistence_fails()
 {
    let port = FakePort {
        apply_finalization_mutation_should_fail: true,
        ..FakePort::with_projects(Vec::new())
    };

    let error = finalize_with(&port)
        .await
        .expect_err("approval should surface persistence failure");

    assert!(matches!(error, UseCaseError::Internal(message) if message == "boom"));
    let calls = port.calls();
    assert!(calls.iter().any(|call| call == "update_op:Applied"));
    assert!(!calls.iter().any(|call| call == "update_op:Reconciled"));
}
