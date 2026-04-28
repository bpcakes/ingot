#![allow(dead_code)]

// Shared route-test helpers are compiled into multiple test binaries, and each binary
// intentionally uses only a subset of them.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
#[allow(unused_imports)]
pub use ingot_domain::test_support::{DEFAULT_TEST_TIMESTAMP, parse_timestamp};
use ingot_store_sqlite::Database;
use ingot_test_support::env::{temp_dir, temp_state_root};
#[allow(unused_imports)]
pub use ingot_test_support::git::{git_output, run_git as git, temp_git_repo, write_file};
#[allow(unused_imports)]
pub use ingot_test_support::http::*;
#[allow(unused_imports)]
pub use ingot_test_support::reports::clean_validation_report;
#[allow(unused_imports)]
pub use ingot_test_support::sqlite::{PersistFixture, migrated_test_db};
use ingot_usecases::{DispatchNotify, ProjectLocks};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tower::ServiceExt;

pub const TS: &str = DEFAULT_TEST_TIMESTAMP;

/// Build a router with an isolated temp state root (avoids production `$HOME/.ingot`).
pub fn test_router(db: Database) -> axum::Router {
    let state_root = temp_state_root("ingot-http-api-state");
    ingot_http_api::build_router_with_project_locks_and_state_root(
        db,
        ProjectLocks::default(),
        state_root,
        DispatchNotify::default(),
    )
}

pub fn expect_status(response: Response, expected: StatusCode) -> Response {
    assert_eq!(
        response.status(),
        expected,
        "unexpected route response status"
    );
    response
}

pub async fn read_json<T>(response: Response) -> T
where
    T: DeserializeOwned,
{
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body");
    serde_json::from_slice(&body).expect("response json")
}

pub async fn get_json<T>(app: axum::Router, uri: impl Into<String>, expected: StatusCode) -> T
where
    T: DeserializeOwned,
{
    let response = route_response(
        app,
        Request::builder()
            .uri(uri.into())
            .body(Body::empty())
            .expect("build GET request"),
    )
    .await;

    read_json(expect_status(response, expected)).await
}

pub async fn post_json<T>(app: axum::Router, uri: impl Into<String>, payload: T) -> Response
where
    T: Serialize,
{
    route_response(
        app,
        Request::builder()
            .method("POST")
            .uri(uri.into())
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&payload).expect("serialize request json"),
            ))
            .expect("build JSON POST request"),
    )
    .await
}

pub async fn post_empty(app: axum::Router, uri: impl Into<String>) -> Response {
    route_response(
        app,
        Request::builder()
            .method("POST")
            .uri(uri.into())
            .body(Body::empty())
            .expect("build empty POST request"),
    )
    .await
}

async fn route_response(app: axum::Router, request: Request<Body>) -> Response {
    app.oneshot(request).await.expect("route response")
}

pub fn fake_codex_probe_script() -> PathBuf {
    let path = temp_dir("ingot-fake-codex").join("codex.sh");
    fs::write(
        &path,
        r#"#!/bin/sh
if [ "$1" = "exec" ] && [ "$2" = "--help" ]; then
  cat <<'EOF'
Usage: codex exec [OPTIONS] [PROMPT] [COMMAND]
      --config <key=value>
  -s, --sandbox <SANDBOX_MODE>
  -C, --cd <DIR>
      --output-schema <FILE>
      --json
  -o, --output-last-message <FILE>
EOF
  exit 0
fi
echo "unexpected arguments: $@" >&2
exit 1
"#,
    )
    .expect("write fake codex");
    let mut permissions = fs::metadata(&path)
        .expect("fake codex metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("chmod fake codex");
    path
}
