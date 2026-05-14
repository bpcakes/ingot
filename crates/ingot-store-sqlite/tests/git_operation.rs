mod common;

use ingot_domain::git_operation::{GitOperationEntityRef, GitOperationStatus, OperationKind};
use ingot_domain::ids::ConvergenceId;
use ingot_domain::ports::RepositoryError;
use ingot_domain::test_support::{GitOperationBuilder, ProjectBuilder, WorkspaceBuilder};
use ingot_domain::workspace::WorkspaceKind;
use ingot_test_support::git::unique_temp_path;
use ingot_test_support::sqlite::PersistFixture;

#[tokio::test]
async fn find_unresolved_finalize_for_convergence_returns_matching_operation() {
    let db = common::migrated_test_db("ingot-store-git-op").await;

    let project = ProjectBuilder::new(unique_temp_path("ingot-store-project")).build();
    db.create_project(&project).await.expect("create project");

    let convergence_id = ConvergenceId::new();
    let operation = GitOperationBuilder::new(
        project.id,
        OperationKind::FinalizeTargetRef,
        GitOperationEntityRef::Convergence(convergence_id),
    )
    .ref_name("refs/heads/main")
    .expected_old_oid("base")
    .new_oid("prepared")
    .commit_oid("prepared")
    .status(GitOperationStatus::Planned)
    .build();
    db.create_git_operation(&operation)
        .await
        .expect("create git operation");

    let found = db
        .find_unresolved_finalize_for_convergence(convergence_id)
        .await
        .expect("find unresolved finalize")
        .expect("matching operation");
    assert_eq!(found.id, operation.id);
}

#[tokio::test]
async fn unique_index_rejects_second_unresolved_finalize_for_same_convergence() {
    let db = common::migrated_test_db("ingot-store-git-op-unique").await;

    let project = ProjectBuilder::new(unique_temp_path("ingot-store-project")).build();
    db.create_project(&project).await.expect("create project");

    let convergence_id = ConvergenceId::new();
    let first = GitOperationBuilder::new(
        project.id,
        OperationKind::FinalizeTargetRef,
        GitOperationEntityRef::Convergence(convergence_id),
    )
    .ref_name("refs/heads/main")
    .expected_old_oid("base")
    .new_oid("prepared")
    .commit_oid("prepared")
    .status(GitOperationStatus::Planned)
    .build();
    db.create_git_operation(&first)
        .await
        .expect("create first operation");

    let second = GitOperationBuilder::new(
        project.id,
        OperationKind::FinalizeTargetRef,
        GitOperationEntityRef::Convergence(convergence_id),
    )
    .ref_name("refs/heads/main")
    .expected_old_oid("base")
    .new_oid("prepared")
    .commit_oid("prepared")
    .status(GitOperationStatus::Applied)
    .build();
    let error = db
        .create_git_operation(&second)
        .await
        .expect_err("second unresolved finalize must conflict");
    assert!(matches!(error, RepositoryError::Conflict(_)));
}

#[tokio::test]
async fn list_latest_failed_prepare_for_convergences_chunks_bind_parameters() {
    let db = common::migrated_test_db("ingot-store-git-op-prepare-chunks").await;

    let project = ProjectBuilder::new(unique_temp_path("ingot-store-project")).build();
    db.create_project(&project).await.expect("create project");
    let workspace = WorkspaceBuilder::new(project.id, WorkspaceKind::Integration)
        .base_commit_oid("base")
        .head_commit_oid("base")
        .build()
        .persist(&db)
        .await
        .expect("create workspace");

    let mut convergence_ids = Vec::new();
    for _ in 0..901 {
        let convergence_id = ConvergenceId::new();
        convergence_ids.push(convergence_id);
        let operation = GitOperationBuilder::new(
            project.id,
            OperationKind::PrepareConvergenceCommit,
            GitOperationEntityRef::Convergence(convergence_id),
        )
        .workspace_id(workspace.id)
        .expected_old_oid("base")
        .status(GitOperationStatus::Failed)
        .metadata(serde_json::json!({
            "source_commit_oids": ["head"],
            "prepared_commit_oids": [],
            "conflict": null
        }))
        .build();
        db.create_git_operation(&operation)
            .await
            .expect("create prepare operation");
    }

    let operations = db
        .list_latest_failed_prepare_for_convergences(&convergence_ids)
        .await
        .expect("list latest prepare operations");

    assert_eq!(operations.len(), convergence_ids.len());
}
