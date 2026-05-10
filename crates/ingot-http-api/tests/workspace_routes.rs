use std::process::Command;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ingot_domain::ids::ProjectId;
use ingot_domain::job::{
    ContextPolicy, ExecutionPermission, JobStatus, OutputArtifactKind, PhaseKind,
};
use ingot_domain::workspace::{RetentionPolicy, WorkspaceKind, WorkspaceStatus};
use ingot_git::project_repo::{ensure_mirror, project_repo_paths};
use ingot_http_api::build_router_with_project_locks_and_state_root;
use ingot_test_support::env::temp_state_root;
use ingot_usecases::{DispatchNotify, ProjectLocks};
use tower::ServiceExt;
use uuid::Uuid;

mod common;
use common::*;

#[tokio::test]
async fn reset_workspace_route_restores_authoring_workspace_head() {
    let repo = temp_git_repo("ingot-http-api");
    let base_commit_oid = git_output(&repo, &["rev-parse", "HEAD"]);
    let workspace_path =
        std::env::temp_dir().join(format!("ingot-http-api-workspace-{}", Uuid::now_v7()));
    git(
        &repo,
        &[
            "update-ref",
            "refs/ingot/workspaces/wrk_reset_test",
            &base_commit_oid,
        ],
    );
    git(
        &repo,
        &[
            "worktree",
            "add",
            "--detach",
            workspace_path.to_str().expect("workspace path"),
            "refs/ingot/workspaces/wrk_reset_test",
        ],
    );
    write_file(&workspace_path.join("tracked.txt"), "changed");

    let db = migrated_test_db("ingot-http-api-db").await;
    let project_id = "prj_00000000000000000000000000000044".to_string();
    let workspace_id = "wrk_00000000000000000000000000000044".to_string();

    test_project_builder(&repo, &project_id)
        .name("Test")
        .build()
        .persist(&db)
        .await
        .expect("insert project");

    test_workspace_builder(&project_id, WorkspaceKind::Authoring, &workspace_id)
        .path(workspace_path.display().to_string())
        .workspace_ref("refs/ingot/workspaces/wrk_reset_test")
        .base_commit_oid(&base_commit_oid)
        .head_commit_oid(&base_commit_oid)
        .retention_policy(RetentionPolicy::Persistent)
        .status(WorkspaceStatus::Ready)
        .build()
        .persist(&db)
        .await
        .expect("insert workspace");

    let app = test_router(db.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/projects/{project_id}/workspaces/{workspace_id}/reset"
                ))
                .method("POST")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("route response");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        git_output(&workspace_path, &["rev-parse", "HEAD"]),
        base_commit_oid
    );
    assert_eq!(
        std::fs::read_to_string(workspace_path.join("tracked.txt")).expect("tracked file"),
        "initial"
    );
}

#[tokio::test]
async fn remove_workspace_route_deletes_abandoned_workspace_ref_and_path() {
    let repo = temp_git_repo("ingot-http-api");
    let head_commit_oid = git_output(&repo, &["rev-parse", "HEAD"]);
    let workspace_path = std::env::temp_dir().join(format!(
        "ingot-http-api-remove-workspace-{}",
        Uuid::now_v7()
    ));

    let db = migrated_test_db("ingot-http-api-db").await;
    let project_id = "prj_00000000000000000000000000000043".to_string();
    let workspace_id = "wrk_00000000000000000000000000000043".to_string();
    let project_uuid = project_id.parse::<ProjectId>().expect("parse project id");
    let state_root = temp_state_root("ingot-http-api-remove-state");
    let paths = project_repo_paths(state_root.as_path(), project_uuid, &repo);
    ensure_mirror(&paths).await.expect("ensure mirror");
    git(
        &paths.mirror_git_dir,
        &[
            "update-ref",
            "refs/ingot/workspaces/wrk_remove_test",
            &head_commit_oid,
        ],
    );
    git(
        &paths.mirror_git_dir,
        &[
            "worktree",
            "add",
            "--detach",
            workspace_path.to_str().expect("workspace path"),
            "refs/ingot/workspaces/wrk_remove_test",
        ],
    );

    test_project_builder(&repo, &project_id)
        .name("Test")
        .build()
        .persist(&db)
        .await
        .expect("insert project");

    let mut workspace = test_workspace_builder(&project_id, WorkspaceKind::Review, &workspace_id)
        .path(workspace_path.display().to_string())
        .workspace_ref("refs/ingot/workspaces/wrk_remove_test")
        .base_commit_oid(&head_commit_oid)
        .head_commit_oid(&head_commit_oid)
        .retention_policy(RetentionPolicy::Ephemeral)
        .status(WorkspaceStatus::Abandoned)
        .build();
    workspace.target_ref = None;
    workspace.persist(&db).await.expect("insert workspace");

    let app = build_router_with_project_locks_and_state_root(
        db.clone(),
        ProjectLocks::default(),
        state_root,
        DispatchNotify::default(),
    );
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/projects/{project_id}/workspaces/{workspace_id}/remove"
                ))
                .method("POST")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("route response");

    assert_eq!(response.status(), StatusCode::OK);
    assert!(!workspace_path.exists());
    let ref_exists = Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            "refs/ingot/workspaces/wrk_remove_test",
        ])
        .current_dir(paths.mirror_git_dir)
        .status()
        .expect("check ref");
    assert!(!ref_exists.success());
}

#[tokio::test]
async fn abandon_workspace_route_rejects_workspace_with_running_job_even_when_status_is_ready() {
    let repo = temp_git_repo("ingot-http-api");
    let head_commit_oid = git_output(&repo, &["rev-parse", "HEAD"]);
    let db = migrated_test_db("ingot-http-api-db").await;
    let project_id = "prj_00000000000000000000000000000045".to_string();
    let item_id = "itm_00000000000000000000000000000045".to_string();
    let revision_id = "rev_00000000000000000000000000000045".to_string();
    let workspace_id = "wrk_00000000000000000000000000000045".to_string();
    let job_id = "job_00000000000000000000000000000045".to_string();

    persist_test_change(
        &db,
        &repo,
        &project_id,
        &item_id,
        &revision_id,
        |builder| builder,
        |builder| builder.explicit_seed(head_commit_oid.as_str()),
    )
    .await;
    persist_test_workspace(
        &db,
        &project_id,
        WorkspaceKind::Authoring,
        &workspace_id,
        |builder| {
            builder
                .created_for_revision_id(parse_id(&revision_id))
                .path(repo.display().to_string())
                .base_commit_oid(&head_commit_oid)
                .head_commit_oid(&head_commit_oid)
                .status(WorkspaceStatus::Ready)
        },
    )
    .await;
    insert_test_job_row(
        &db,
        TestJobInsert {
            id: &job_id,
            project_id: &project_id,
            item_id: &item_id,
            item_revision_id: &revision_id,
            step_id: "validate_candidate_initial",
            status: JobStatus::Running,
            workspace_id: Some(&workspace_id),
            phase_kind: PhaseKind::Validate,
            workspace_kind: WorkspaceKind::Authoring,
            execution_permission: ExecutionPermission::DaemonOnly,
            context_policy: ContextPolicy::None,
            phase_template_slug: "",
            output_artifact_kind: OutputArtifactKind::ValidationReport,
            job_input: TestJobInput::CandidateSubject(&head_commit_oid, &head_commit_oid),
            created_at: TS,
            started_at: Some(TS),
            ..TestJobInsert::new(
                &job_id,
                &project_id,
                &item_id,
                &revision_id,
                "validate_candidate_initial",
            )
        },
    )
    .await;

    let app = test_router(db);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/projects/{project_id}/workspaces/{workspace_id}/abandon"
                ))
                .method("POST")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("route response");

    assert_eq!(response.status(), StatusCode::CONFLICT);
}
