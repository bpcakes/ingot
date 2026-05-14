mod common;

use ingot_domain::activity::{Activity, ActivityEventType, ActivitySubject};
use ingot_domain::commit_oid::CommitOid;
use ingot_domain::convergence::ConvergenceStatus;
use ingot_domain::convergence_queue::ConvergenceQueueEntryStatus;
use ingot_domain::git_operation::{
    ConvergenceConflictFile, ConvergenceConflictMetadata, ConvergenceConflictStage,
    ConvergenceReplayMetadata, GitOperationEntityRef, GitOperationStatus,
    MAX_CONVERGENCE_CONFLICT_GIT_ERROR_BYTES, OperationKind,
};
use ingot_domain::ids::{ActivityId, ItemId};
use ingot_domain::item::{ApprovalState, Escalation, EscalationReason};
use ingot_domain::ports::{
    ItemEscalationPatch, PrepareConvergenceFailureMutation, RepositoryError,
};
use ingot_domain::test_support::{
    ConvergenceBuilder, ConvergenceQueueEntryBuilder, GitOperationBuilder, ItemBuilder,
    ProjectBuilder, RevisionBuilder, WorkspaceBuilder,
};
use ingot_domain::workspace::{WorkspaceKind, WorkspaceStatus};
use ingot_test_support::sqlite::PersistFixture;

#[tokio::test]
async fn prepare_failure_persists_all_success_mutations() {
    let db = common::migrated_test_db("prepare-failure-success").await;

    let project = ProjectBuilder::new("/tmp/test")
        .name("Test")
        .build()
        .persist(&db)
        .await
        .expect("create project");
    let revision = RevisionBuilder::new(ItemId::new())
        .seed_commit_oid(Some("abc"))
        .seed_target_commit_oid(Some("def"))
        .build();
    let item = ItemBuilder::new(project.id, revision.id)
        .id(revision.item_id)
        .approval_state(ApprovalState::Pending)
        .build();
    let (item, revision) = (item, revision)
        .persist(&db)
        .await
        .expect("create item with revision");

    let integration_workspace = WorkspaceBuilder::new(project.id, WorkspaceKind::Integration)
        .created_for_revision_id(revision.id)
        .status(WorkspaceStatus::Ready)
        .base_commit_oid("target")
        .head_commit_oid("target")
        .build()
        .persist(&db)
        .await
        .expect("create integration workspace");
    let source_workspace = WorkspaceBuilder::new(project.id, WorkspaceKind::Authoring)
        .created_for_revision_id(revision.id)
        .status(WorkspaceStatus::Ready)
        .base_commit_oid("abc")
        .head_commit_oid("head")
        .build()
        .persist(&db)
        .await
        .expect("create source workspace");

    let convergence = ConvergenceBuilder::new(project.id, item.id, revision.id)
        .source_workspace_id(source_workspace.id)
        .integration_workspace_id(integration_workspace.id)
        .source_head_commit_oid("head")
        .status(ConvergenceStatus::Running)
        .input_target_commit_oid("target")
        .no_prepared_commit_oid()
        .build()
        .persist(&db)
        .await
        .expect("create convergence");
    let queue_entry = ConvergenceQueueEntryBuilder::new(project.id, item.id, revision.id).build();
    db.create_queue_entry(&queue_entry)
        .await
        .expect("create queue entry");
    let operation = GitOperationBuilder::new(
        project.id,
        OperationKind::PrepareConvergenceCommit,
        GitOperationEntityRef::Convergence(convergence.id),
    )
    .workspace_id(integration_workspace.id)
    .expected_old_oid("target")
    .status(GitOperationStatus::Planned)
    .metadata(serde_json::json!({
        "source_commit_oids": ["head"],
        "prepared_commit_oids": [],
        "conflict": null
    }))
    .build();
    db.create_git_operation(&operation)
        .await
        .expect("create git operation");

    let mut failed_workspace = integration_workspace.clone();
    failed_workspace.mark_error(chrono::Utc::now());

    let mut failed_convergence = convergence.clone();
    failed_convergence
        .transition_to_conflicted("tracked.txt conflicted".into(), chrono::Utc::now())
        .expect("transition convergence");

    let item_escalation = ItemEscalationPatch {
        id: item.id,
        approval_state: ApprovalState::NotRequested,
        escalation: Escalation::OperatorRequired {
            reason: EscalationReason::ConvergenceConflict,
        },
        updated_at: chrono::Utc::now(),
    };

    let mut released_queue = queue_entry.clone();
    released_queue.status = ConvergenceQueueEntryStatus::Released;
    released_queue.released_at = Some(chrono::Utc::now());
    released_queue.updated_at = chrono::Utc::now();

    let mut failed_operation = operation.clone();
    failed_operation.status = GitOperationStatus::Failed;
    failed_operation.completed_at = Some(chrono::Utc::now());
    failed_operation
        .payload
        .set_replay_metadata(ConvergenceReplayMetadata {
            source_commit_oids: vec![CommitOid::new("head")],
            prepared_commit_oids: Vec::new(),
            conflict: Some(ConvergenceConflictMetadata {
                failed_source_commit_oid: CommitOid::new("head"),
                git_error: "x".repeat(3_000),
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
            }),
        })
        .expect("set replay metadata");

    let activities = vec![
        Activity {
            id: ActivityId::new(),
            project_id: project.id,
            event_type: ActivityEventType::ConvergenceConflicted,
            subject: ActivitySubject::Convergence(convergence.id),
            payload: serde_json::json!({ "item_id": item.id, "summary": "tracked.txt conflicted" }),
            created_at: chrono::Utc::now(),
        },
        Activity {
            id: ActivityId::new(),
            project_id: project.id,
            event_type: ActivityEventType::ItemEscalated,
            subject: ActivitySubject::Item(item.id),
            payload: serde_json::json!({ "reason": "convergence_conflict" }),
            created_at: chrono::Utc::now(),
        },
    ];

    db.apply_prepare_convergence_failure(PrepareConvergenceFailureMutation {
        workspace: failed_workspace,
        convergence: failed_convergence,
        item: item_escalation,
        queue_entry: released_queue,
        git_operation: failed_operation,
        activities,
    })
    .await
    .expect("apply prepare failure");

    let persisted_workspace = db
        .get_workspace(integration_workspace.id)
        .await
        .expect("workspace");
    assert_eq!(persisted_workspace.state.status(), WorkspaceStatus::Error);

    let persisted_convergence = db
        .get_convergence(convergence.id)
        .await
        .expect("convergence");
    assert_eq!(
        persisted_convergence.state.status(),
        ConvergenceStatus::Conflicted
    );
    assert_eq!(
        persisted_convergence.state.conflict_summary(),
        Some("tracked.txt conflicted")
    );

    let persisted_item = db.get_item(item.id).await.expect("item");
    assert_eq!(persisted_item.approval_state, ApprovalState::NotRequested);
    assert_eq!(
        persisted_item.escalation,
        Escalation::OperatorRequired {
            reason: EscalationReason::ConvergenceConflict
        }
    );

    let persisted_queue = db.get_queue_entry(queue_entry.id).await.expect("queue");
    assert_eq!(
        persisted_queue.status,
        ConvergenceQueueEntryStatus::Released
    );
    assert!(persisted_queue.released_at.is_some());

    let (status, metadata): (String, String) =
        sqlx::query_as("SELECT status, metadata FROM git_operations WHERE id = ?")
            .bind(operation.id.to_string())
            .fetch_one(db.raw_pool())
            .await
            .expect("git operation");
    assert_eq!(status, "failed");
    let metadata: ConvergenceReplayMetadata =
        serde_json::from_str(&metadata).expect("replay metadata");
    let conflict = metadata.conflict.expect("conflict metadata");
    assert!(conflict.git_error.len() <= MAX_CONVERGENCE_CONFLICT_GIT_ERROR_BYTES);
    assert!(conflict.git_error.ends_with("\n[truncated]"));
    assert_eq!(conflict.total_file_count, 1);
    assert_eq!(conflict.files[0].path, "tracked.txt");

    let activity = db
        .list_activity_by_project(project.id, 20, 0)
        .await
        .expect("activity");
    assert_eq!(activity.len(), 2);
    assert!(
        activity
            .iter()
            .any(|row| row.event_type == ActivityEventType::ConvergenceConflicted)
    );
    assert!(
        activity
            .iter()
            .any(|row| row.event_type == ActivityEventType::ItemEscalated)
    );
}

#[tokio::test]
async fn prepare_failure_rolls_back_when_git_operation_update_fails() {
    let db = common::migrated_test_db("prepare-failure-rollback").await;

    let project = ProjectBuilder::new("/tmp/test")
        .name("Test")
        .build()
        .persist(&db)
        .await
        .expect("create project");
    let revision = RevisionBuilder::new(ItemId::new())
        .seed_commit_oid(Some("abc"))
        .seed_target_commit_oid(Some("def"))
        .build();
    let item = ItemBuilder::new(project.id, revision.id)
        .id(revision.item_id)
        .approval_state(ApprovalState::Pending)
        .build();
    let (item, revision) = (item, revision)
        .persist(&db)
        .await
        .expect("create item with revision");

    let integration_workspace = WorkspaceBuilder::new(project.id, WorkspaceKind::Integration)
        .created_for_revision_id(revision.id)
        .status(WorkspaceStatus::Ready)
        .base_commit_oid("target")
        .head_commit_oid("target")
        .build()
        .persist(&db)
        .await
        .expect("create integration workspace");
    let source_workspace = WorkspaceBuilder::new(project.id, WorkspaceKind::Authoring)
        .created_for_revision_id(revision.id)
        .status(WorkspaceStatus::Ready)
        .base_commit_oid("abc")
        .head_commit_oid("head")
        .build()
        .persist(&db)
        .await
        .expect("create source workspace");

    let convergence = ConvergenceBuilder::new(project.id, item.id, revision.id)
        .source_workspace_id(source_workspace.id)
        .integration_workspace_id(integration_workspace.id)
        .source_head_commit_oid("head")
        .status(ConvergenceStatus::Running)
        .input_target_commit_oid("target")
        .no_prepared_commit_oid()
        .build()
        .persist(&db)
        .await
        .expect("create convergence");
    let queue_entry = ConvergenceQueueEntryBuilder::new(project.id, item.id, revision.id).build();
    db.create_queue_entry(&queue_entry)
        .await
        .expect("create queue entry");

    let mut failed_workspace = integration_workspace.clone();
    failed_workspace.mark_error(chrono::Utc::now());

    let mut failed_convergence = convergence.clone();
    failed_convergence
        .transition_to_conflicted("tracked.txt conflicted".into(), chrono::Utc::now())
        .expect("transition convergence");

    let item_escalation = ItemEscalationPatch {
        id: item.id,
        approval_state: ApprovalState::NotRequested,
        escalation: Escalation::OperatorRequired {
            reason: EscalationReason::ConvergenceConflict,
        },
        updated_at: chrono::Utc::now(),
    };

    let mut released_queue = queue_entry.clone();
    released_queue.status = ConvergenceQueueEntryStatus::Released;
    released_queue.released_at = Some(chrono::Utc::now());
    released_queue.updated_at = chrono::Utc::now();

    let missing_operation = GitOperationBuilder::new(
        project.id,
        OperationKind::PrepareConvergenceCommit,
        GitOperationEntityRef::Convergence(convergence.id),
    )
    .workspace_id(integration_workspace.id)
    .expected_old_oid("target")
    .status(GitOperationStatus::Failed)
    .metadata(serde_json::json!({
        "source_commit_oids": ["head"],
        "prepared_commit_oids": [],
        "conflict": null
    }))
    .completed_at(chrono::Utc::now())
    .build();

    let mutation = PrepareConvergenceFailureMutation {
        workspace: failed_workspace,
        convergence: failed_convergence,
        item: item_escalation,
        queue_entry: released_queue,
        git_operation: missing_operation,
        activities: vec![Activity {
            id: ActivityId::new(),
            project_id: project.id,
            event_type: ActivityEventType::ConvergenceConflicted,
            subject: ActivitySubject::Convergence(convergence.id),
            payload: serde_json::json!({ "item_id": item.id }),
            created_at: chrono::Utc::now(),
        }],
    };

    let error = db
        .apply_prepare_convergence_failure(mutation)
        .await
        .expect_err("missing operation should fail");
    assert!(matches!(error, RepositoryError::NotFound));

    let persisted_workspace = db
        .get_workspace(integration_workspace.id)
        .await
        .expect("workspace");
    assert_eq!(persisted_workspace.state.status(), WorkspaceStatus::Ready);

    let persisted_convergence = db
        .get_convergence(convergence.id)
        .await
        .expect("convergence");
    assert_eq!(
        persisted_convergence.state.status(),
        ConvergenceStatus::Running
    );

    let persisted_item = db.get_item(item.id).await.expect("item");
    assert_eq!(persisted_item.approval_state, ApprovalState::Pending);
    assert_eq!(persisted_item.escalation, Escalation::None);

    let persisted_queue = db.get_queue_entry(queue_entry.id).await.expect("queue");
    assert_eq!(persisted_queue.status, ConvergenceQueueEntryStatus::Head);

    let activity = db
        .list_activity_by_project(project.id, 20, 0)
        .await
        .expect("activity");
    assert!(
        activity.is_empty(),
        "activity insert should roll back with the failed mutation"
    );
}
