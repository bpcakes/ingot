use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use ingot_usecases::{UseCaseError, UseCaseInfraError};
use serde_json::json;

#[derive(Debug)]
pub enum ApiError {
    UseCase(UseCaseError),
    BadRequest { code: &'static str, message: String },
    Conflict { code: &'static str, message: String },
    NotFound { code: &'static str, message: String },
    Validation { message: String },
    Internal { message: String },
}

impl ApiError {
    pub fn invalid_id(entity: impl AsRef<str>, value: &str) -> Self {
        let entity = entity.as_ref();
        Self::BadRequest {
            code: "invalid_id",
            message: format!("Invalid {entity} id: {value}"),
        }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation {
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            ApiError::UseCase(use_case_error) => match use_case_error {
                UseCaseError::ProjectNotFound => (
                    StatusCode::NOT_FOUND,
                    "project_not_found",
                    "Project not found".into(),
                ),
                UseCaseError::ItemNotFound => (
                    StatusCode::NOT_FOUND,
                    "item_not_found",
                    "Item not found".into(),
                ),
                UseCaseError::ItemNotOpen => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "item_not_open",
                    "Item is not open".into(),
                ),
                UseCaseError::ItemNotIdle => (
                    StatusCode::CONFLICT,
                    "item_not_idle",
                    "Item is not idle".into(),
                ),
                UseCaseError::ItemNotDeferred => (
                    StatusCode::CONFLICT,
                    "item_not_deferred",
                    "Item is not deferred".into(),
                ),
                UseCaseError::ItemNotReopenable => (
                    StatusCode::CONFLICT,
                    "item_not_reopenable",
                    "Only dismissed or invalidated items can be reopened".into(),
                ),
                UseCaseError::PendingApprovalCannotDefer => (
                    StatusCode::CONFLICT,
                    "item_pending_approval",
                    "Pending approval items cannot be deferred".into(),
                ),
                UseCaseError::ApprovalNotPending => (
                    StatusCode::CONFLICT,
                    "approval_not_pending",
                    "Approval is not pending".into(),
                ),
                UseCaseError::ConvergenceNotPreparable => (
                    StatusCode::CONFLICT,
                    "convergence_not_preparable",
                    "Convergence cannot be prepared in the current item state".into(),
                ),
                UseCaseError::ConvergenceNotQueued => (
                    StatusCode::CONFLICT,
                    "convergence_not_queued",
                    "A lane head is required before approval can be granted".into(),
                ),
                UseCaseError::ConvergenceNotLaneHead => (
                    StatusCode::CONFLICT,
                    "convergence_not_lane_head",
                    "Only the target-ref lane head can be approved".into(),
                ),
                UseCaseError::JobNotActive => (
                    StatusCode::CONFLICT,
                    "job_not_active",
                    "Job is not active".into(),
                ),
                UseCaseError::FindingNotFound => (
                    StatusCode::NOT_FOUND,
                    "finding_not_found",
                    "Finding not found".into(),
                ),
                UseCaseError::FindingNotTriageable => (
                    StatusCode::CONFLICT,
                    "finding_not_triageable",
                    "Finding is not triageable".into(),
                ),
                UseCaseError::FindingSubjectUnreachable => (
                    StatusCode::CONFLICT,
                    "finding_subject_unreachable",
                    "Finding subject is unreachable".into(),
                ),
                UseCaseError::InvalidFindingTriage(message) => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid_finding_triage",
                    message,
                ),
                UseCaseError::IllegalStepDispatch(message) => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "illegal_step_dispatch",
                    message,
                ),
                UseCaseError::ActiveJobExists => (
                    StatusCode::CONFLICT,
                    "active_job_exists",
                    "Active job exists".into(),
                ),
                UseCaseError::ActiveConvergenceExists => (
                    StatusCode::CONFLICT,
                    "active_convergence_exists",
                    "Active convergence exists".into(),
                ),
                UseCaseError::CompletedItemCannotReopen => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "completed_item_cannot_reopen",
                    "Completed items cannot be reopened".into(),
                ),
                UseCaseError::InvalidTargetRef(target_ref) => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid_target_ref",
                    format!("Target ref must be a branch under refs/heads/*: {target_ref}"),
                ),
                UseCaseError::TargetRefUnresolved(target_ref) => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "target_ref_unresolved",
                    format!("Target ref could not be resolved: {target_ref}"),
                ),
                UseCaseError::RevisionSeedUnreachable(seed_name) => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "revision_seed_unreachable",
                    format!("Revision seed is not reachable: {seed_name}"),
                ),
                UseCaseError::LinkedItemNotFound => (
                    StatusCode::NOT_FOUND,
                    "linked_item_not_found",
                    "Linked item not found".into(),
                ),
                UseCaseError::LinkedItemProjectMismatch => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "linked_item_project_mismatch",
                    "Linked item must belong to the same project".into(),
                ),
                UseCaseError::PreparedConvergenceMissing => (
                    StatusCode::CONFLICT,
                    "prepared_convergence_missing",
                    "No prepared convergence exists".into(),
                ),
                UseCaseError::PreparedConvergenceStale => (
                    StatusCode::CONFLICT,
                    "prepared_convergence_stale",
                    "Prepared convergence is stale".into(),
                ),
                UseCaseError::ProtocolViolation(message) => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "protocol_violation",
                    message,
                ),
                UseCaseError::Repository(_) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "Internal error".into(),
                ),
                UseCaseError::Infrastructure(error) => infrastructure_error_response(error),
                UseCaseError::Internal(message) => {
                    (StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
                }
            },
            ApiError::Conflict { code, message } => (StatusCode::CONFLICT, code, message),
            ApiError::NotFound { code, message } => (StatusCode::NOT_FOUND, code, message),
            ApiError::BadRequest { code, message } => (StatusCode::BAD_REQUEST, code, message),
            ApiError::Validation { message } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_error",
                message,
            ),
            ApiError::Internal { message } => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
            }
        };

        let body = json!({
            "error": {
                "code": code,
                "message": message,
            }
        });

        (status, axum::Json(body)).into_response()
    }
}

fn infrastructure_error_response(error: UseCaseInfraError) -> (StatusCode, &'static str, String) {
    match error {
        UseCaseInfraError::WorkspaceBusy { source } => {
            (StatusCode::CONFLICT, "workspace_busy", source.to_string())
        }
        UseCaseInfraError::WorkspaceStateMismatch { source } => (
            StatusCode::CONFLICT,
            "workspace_state_mismatch",
            source.to_string(),
        ),
        UseCaseInfraError::Git { .. } => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "git_operation_failed",
            "Git operation failed".into(),
        ),
        UseCaseInfraError::WorkspaceInvalidState { .. }
        | UseCaseInfraError::Io { .. }
        | UseCaseInfraError::Serialization { .. }
        | UseCaseInfraError::External { .. } => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Internal error".into(),
        ),
    }
}

impl From<UseCaseError> for ApiError {
    fn from(error: UseCaseError) -> Self {
        Self::UseCase(error)
    }
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use serde_json::Value;

    use super::*;

    async fn response_error_json(error: ApiError) -> (StatusCode, Value) {
        let response = error.into_response();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        let json = serde_json::from_slice(&body).expect("json body");
        (status, json)
    }

    #[tokio::test]
    async fn git_infrastructure_errors_are_redacted_http_500() {
        let (status, json) = response_error_json(ApiError::from(UseCaseError::Infrastructure(
            UseCaseInfraError::git(ingot_git::commands::GitCommandError::operation_failed(
                "test",
                "sensitive stderr",
            )),
        )))
        .await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(json["error"]["code"], "git_operation_failed");
        assert_eq!(json["error"]["message"], "Git operation failed");
    }

    #[tokio::test]
    async fn workspace_busy_errors_map_to_http_409() {
        let (status, json) = response_error_json(ApiError::from(UseCaseError::Infrastructure(
            UseCaseInfraError::workspace_busy(ingot_workspace::WorkspaceError::Busy),
        )))
        .await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(json["error"]["code"], "workspace_busy");
    }
}
