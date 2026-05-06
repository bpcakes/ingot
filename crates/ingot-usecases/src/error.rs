use std::error::Error;

use ingot_domain::git_ref::TargetRefParseError;

pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

#[derive(Debug, thiserror::Error)]
pub enum UseCaseInfraError {
    #[error("git infrastructure error: {source}")]
    Git {
        #[source]
        source: BoxError,
    },
    #[error("workspace is busy: {source}")]
    WorkspaceBusy {
        #[source]
        source: BoxError,
    },
    #[error("workspace state mismatch: {source}")]
    WorkspaceStateMismatch {
        #[source]
        source: BoxError,
    },
    #[error("workspace invalid state: {source}")]
    WorkspaceInvalidState {
        #[source]
        source: BoxError,
    },
    #[error("io infrastructure error: {source}")]
    Io {
        #[source]
        source: BoxError,
    },
    #[error("serialization infrastructure error: {source}")]
    Serialization {
        #[source]
        source: BoxError,
    },
    #[error("{category} infrastructure error: {source}")]
    External {
        category: &'static str,
        #[source]
        source: BoxError,
    },
}

impl UseCaseInfraError {
    #[must_use]
    pub fn git(source: impl Error + Send + Sync + 'static) -> Self {
        Self::Git {
            source: boxed_error(source),
        }
    }

    #[must_use]
    pub fn workspace_busy(source: impl Error + Send + Sync + 'static) -> Self {
        Self::WorkspaceBusy {
            source: boxed_error(source),
        }
    }

    #[must_use]
    pub fn workspace_state_mismatch(source: impl Error + Send + Sync + 'static) -> Self {
        Self::WorkspaceStateMismatch {
            source: boxed_error(source),
        }
    }

    #[must_use]
    pub fn workspace_invalid_state(source: impl Error + Send + Sync + 'static) -> Self {
        Self::WorkspaceInvalidState {
            source: boxed_error(source),
        }
    }

    #[must_use]
    pub fn io(source: impl Error + Send + Sync + 'static) -> Self {
        Self::Io {
            source: boxed_error(source),
        }
    }

    #[must_use]
    pub fn serialization(source: impl Error + Send + Sync + 'static) -> Self {
        Self::Serialization {
            source: boxed_error(source),
        }
    }

    #[must_use]
    pub fn external(category: &'static str, source: impl Error + Send + Sync + 'static) -> Self {
        Self::External {
            category,
            source: boxed_error(source),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UseCaseError {
    #[error("project not found")]
    ProjectNotFound,
    #[error("item not found")]
    ItemNotFound,
    #[error("item not open")]
    ItemNotOpen,
    #[error("item not idle")]
    ItemNotIdle,
    #[error("item is not deferred")]
    ItemNotDeferred,
    #[error("item is not reopenable")]
    ItemNotReopenable,
    #[error("pending approval items cannot be deferred")]
    PendingApprovalCannotDefer,
    #[error("approval not pending")]
    ApprovalNotPending,
    #[error("convergence is not preparable")]
    ConvergenceNotPreparable,
    #[error("convergence is not queued")]
    ConvergenceNotQueued,
    #[error("convergence is not lane head")]
    ConvergenceNotLaneHead,
    #[error("job is not active")]
    JobNotActive,
    #[error("finding not found")]
    FindingNotFound,
    #[error("finding is not triageable")]
    FindingNotTriageable,
    #[error("finding subject is unreachable")]
    FindingSubjectUnreachable,
    #[error("invalid finding triage: {0}")]
    InvalidFindingTriage(String),
    #[error("illegal step dispatch: {0}")]
    IllegalStepDispatch(String),
    #[error("active job exists")]
    ActiveJobExists,
    #[error("active convergence exists")]
    ActiveConvergenceExists,
    #[error("completed item cannot reopen")]
    CompletedItemCannotReopen,
    #[error("invalid target ref: {0}")]
    InvalidTargetRef(String),
    #[error("target ref unresolved: {0}")]
    TargetRefUnresolved(String),
    #[error("revision seed unreachable: {0}")]
    RevisionSeedUnreachable(String),
    #[error("linked item not found")]
    LinkedItemNotFound,
    #[error("linked item must belong to the same project")]
    LinkedItemProjectMismatch,
    #[error("prepared convergence missing")]
    PreparedConvergenceMissing,
    #[error("prepared convergence stale")]
    PreparedConvergenceStale,
    #[error("protocol violation: {0}")]
    ProtocolViolation(String),
    #[error("repository error: {0}")]
    Repository(#[from] ingot_domain::ports::RepositoryError),
    #[error("infrastructure error: {0}")]
    Infrastructure(#[from] UseCaseInfraError),
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<TargetRefParseError> for UseCaseError {
    fn from(error: TargetRefParseError) -> Self {
        Self::InvalidTargetRef(error.input().to_string())
    }
}

fn boxed_error(source: impl Error + Send + Sync + 'static) -> BoxError {
    Box::new(source)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::{UseCaseError, UseCaseInfraError};

    #[test]
    fn infrastructure_error_preserves_source_chain() {
        let source = std::io::Error::other("disk unavailable");
        let error = UseCaseError::Infrastructure(UseCaseInfraError::io(source));

        let UseCaseError::Infrastructure(UseCaseInfraError::Io { .. }) = &error else {
            panic!("expected io infrastructure classification");
        };
        assert_eq!(
            error
                .source()
                .and_then(Error::source)
                .map(ToString::to_string)
                .as_deref(),
            Some("disk unavailable")
        );
    }
}
