# Move Dispatch Orchestration Into Usecases

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds.

This document follows `.agent/PLANS.md`.

## Purpose / Big Picture

Dispatching a job currently spreads behavior across HTTP route code and the usecase crate. After this change, HTTP routes will load request data, call usecase functions, map errors, and return responses, while `ingot-usecases::dispatch` owns job-input binding, authoring workspace provisioning, investigation ref planning, job persistence, rollback cleanup, and dispatch activity persistence. The observable behavior should not change: dispatch and retry endpoints still create queued jobs, fill the same inputs, create the same side effects, and clean up failed side effects.

## Progress

- [x] (2026-04-28) Inspected current HTTP dispatch route and `ingot-usecases::dispatch` boundaries.
- [x] (2026-04-28) Move orchestration into `ingot-usecases::dispatch`.
- [x] (2026-04-28) Simplify HTTP dispatch route to delegate to the new usecase.
- [x] (2026-04-28) Move or adapt focused tests for binding and persistence behavior.
- [x] (2026-04-28) Run focused Rust dispatch checks.
- [x] (2026-04-28) Run the wider repository check and record results.

## Surprises & Discoveries

- Observation: `crates/ingot-usecases/src/dispatch.rs` already had local uncommitted edits that factor review and validation auto-dispatch through `auto_dispatch_closure_relevant_step`.
  Evidence: `git diff -- crates/ingot-usecases/src/dispatch.rs` showed only that pre-existing refactor before this plan was written.

## Decision Log

- Decision: Preserve the existing uncommitted auto-dispatch refactor while adding the new dispatch orchestration.
  Rationale: It appears unrelated but valid work in the same file; repository instructions require working with user changes instead of reverting them.
  Date/Author: 2026-04-28 / Codex

- Decision: Keep step selection in HTTP for this refactor and move only binding, persistence, cleanup, workspace provisioning, investigation refs, and activity persistence.
  Rationale: The requested low-risk move targets orchestration already duplicated in HTTP; moving step selection would broaden the behavioral surface.
  Date/Author: 2026-04-28 / Codex

## Outcomes & Retrospective

Dispatch orchestration now lives in `ingot-usecases::dispatch`, while HTTP routes delegate persistence, workspace handling, investigation refs, cleanup, and activity creation to the usecase layer. Focused dispatch and retry tests pass, and `make check` passes.

## Context and Orientation

The HTTP route module `crates/ingot-http-api/src/router/dispatch.rs` exposes endpoints for creating and retrying jobs. It currently constructs a `Job`, then mutates it by binding missing `JobInput`, possibly creates an authoring workspace, possibly plans an investigation git ref, persists the job, rolls back some failed side effects, ensures a workspace, and appends activity. That orchestration is usecase behavior.

The usecase module `crates/ingot-usecases/src/dispatch.rs` already defines `DispatchInfraPort` for git and filesystem side effects and owns helpers such as `cleanup_failed_dispatch` and `apply_pending_investigation_ref_or_cleanup`. The HTTP adapter `crates/ingot-http-api/src/router/infra_ports.rs` bridges repository-independent usecase ports to concrete git/workspace infrastructure.

## Plan of Work

Add a usecase-level `prepare_and_persist_dispatched_job` function that accepts repository ports, the dispatch infra port, loaded project/item/revision/job history, a mutable `Job`, and activity metadata. It will perform all binding and persistence behavior currently in HTTP. Add an infra-port method for ensuring authoring workspace state because usecases must not depend on `ingot-workspace` directly.

Update the HTTP route to call the new function after `dispatch_job` or `retry_job` creates a domain job. Remove the route-local binding and persistence helpers. Keep route-local loading, locking, projected dispatch evaluation, previous-job lookup, and response wrapping.

Move binding-specific tests into usecase tests where practical. Keep HTTP cleanup tests that need the real `HttpInfraAdapter`, git mirror, and filesystem cleanup.

## Concrete Steps

From `/Users/aa/Documents/ingot`, edit:

- `crates/ingot-usecases/src/dispatch.rs`
- `crates/ingot-http-api/src/router/infra_ports.rs`
- `crates/ingot-http-api/src/router/dispatch.rs`

Then run:

    cargo test -p ingot-usecases dispatch
    cargo test -p ingot-http-api dispatch
    make check

## Validation and Acceptance

The focused usecase dispatch tests should prove that `InvestigateItem` binding can return a pending investigation ref without persisting it before job creation, that partial workspace state falls back correctly for investigation dispatch, and that incomplete candidate subjects are rejected for review dispatch.

The HTTP dispatch tests should continue to prove cleanup side effects through `HttpInfraAdapter` and route-level dispatch/retry behavior. `make check` should pass for the workspace.

## Idempotence and Recovery

The refactor is source-only and can be retried by re-running the same tests after any failure. If a test fails because of changed payload shape or rollback behavior, preserve the previous HTTP-observable behavior unless the failure exposes orchestration that was intentionally moved.

## Artifacts and Notes

Focused validation completed:

    cargo test -p ingot-usecases dispatch
    result: ok. 14 passed; 0 failed

    cargo test -p ingot-http-api dispatch
    result: ok. dispatch-filtered HTTP tests passed across lib and route integration tests

    cargo test -p ingot-http-api retry_route
    result: ok. 3 passed; 0 failed

    make check
    result: ok. cargo check completed successfully

## Interfaces and Dependencies

At completion, `ingot-usecases::dispatch` must expose:

    pub struct DispatchActivityContext {
        pub dispatch_origin: Option<&'static str>,
        pub supersedes_job_id: Option<JobId>,
        pub retry_no: Option<u32>,
    }

    pub struct PreparedDispatchedJob {
        pub job: Job,
    }

    pub async fn prepare_and_persist_dispatched_job<J, W, GO, A, G>(
        job_repo: &J,
        workspace_repo: &W,
        git_op_repo: &GO,
        activity_repo: &A,
        git_port: &G,
        project: &Project,
        item: &Item,
        revision: &ItemRevision,
        jobs: &[Job],
        job: Job,
        activity: DispatchActivityContext,
    ) -> Result<PreparedDispatchedJob, UseCaseError>

`DispatchInfraPort` must gain an `ensure_authoring_workspace` method that receives any existing authoring workspace and returns the provisioned workspace. The HTTP adapter remains the only layer that calls `ingot-workspace`.
