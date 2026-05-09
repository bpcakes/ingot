# Consolidate usecase workflow entry points

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. This document follows `.agent/PLANS.md`.

## Purpose / Big Picture

This refactor makes the `ingot-usecases` workflow modules easier to navigate without changing runtime behavior. After it is complete, contributors should not have to jump between separate top-level entry points for one dispatch pipeline, one job-end lifecycle, or one convergence prepare-to-finalize flow. The proof is source structure plus unchanged behavior: the public `ingot_usecases::job` and `ingot_usecases::convergence` APIs still compile for downstream crates, and the focused usecase and dependent crate tests pass.

## Progress

- [x] (2026-05-09 00:00Z) Read `.agent/PLANS.md`, inspected current working tree state, and mapped the current public surfaces of `dispatch.rs`, `job_dispatch.rs`, `job_lifecycle.rs`, `job_completion.rs`, and `convergence/*`.
- [x] (2026-05-09 00:10Z) Consolidated dispatch so `DispatchJobCommand`, `dispatch_job`, and `retry_job` are re-exported by `crate::dispatch` and `crate::job`, and removed `mod job_dispatch` from `src/lib.rs`.
- [x] (2026-05-09 00:10Z) Consolidated job-end handling so non-success termination functions are exported through `crate::job`, removed `pub mod job_lifecycle` from `src/lib.rs`, and updated downstream callers.
- [x] (2026-05-09 00:30Z) Tightened the dispatch and job-end consolidation by embedding the former `job_dispatch.rs` code into `dispatch.rs` and the former `job_lifecycle.rs` code into `job_completion.rs`; there are no replacement `dispatch/planning.rs` or `job/termination.rs` files.
- [x] (2026-05-09 00:22Z) Collapsed convergence production modules from `command.rs`, `context.rs`, `finalization.rs`, `system_actions.rs`, and `types.rs` into `flow.rs` and `service.rs` while preserving `crate::convergence` public re-exports.
- [x] (2026-05-09 00:35Z) Ran `cargo fmt --all`, `cargo test -p ingot-usecases`, and `cargo check -p ingot-http-api -p ingot-agent-runtime --all-targets`.
- [x] (2026-05-09 00:40Z) Ran `cargo clippy -p ingot-usecases -p ingot-http-api -p ingot-agent-runtime --all-targets -- -D warnings`.
- [x] (2026-05-09 00:42Z) Ran `cargo fmt --all --check`, `git diff --check`, and final structure searches.

## Surprises & Discoveries

- Observation: The tree already has convergence-related modifications before this plan starts, including `crates/ingot-usecases/src/convergence/context.rs` and HTTP router changes that call the new shared context helpers.
  Evidence: `git status --short` shows modified convergence router files and an untracked `convergence/context.rs`.

- Observation: `ingot_usecases::job` is already the stable facade for successful completion and pure dispatch, but non-success job termination is still reached through the separate public module `ingot_usecases::job_lifecycle`.
  Evidence: `crates/ingot-usecases/src/job.rs` re-exports completion and dispatch items, while `crates/ingot-usecases/src/lib.rs` has `pub mod job_lifecycle`.

- Observation: Removing the top-level job modules did not require behavior changes.
  Evidence: `cargo check -p ingot-usecases --all-targets` passed after moving exports through `crate::dispatch` and `crate::job`.

- Observation: The convergence production merge was mostly mechanical because the existing public facade already isolated downstream callers from file layout.
  Evidence: `cargo check -p ingot-usecases --all-targets` passed after replacing the five production convergence modules with `flow.rs` and `service.rs`.

- Observation: The stricter completion audit showed that moving the old dispatch and lifecycle files into owned folders was not enough because it preserved replacement file-level splits.
  Evidence: the final structure check now has no `job_dispatch.rs`, `job_lifecycle.rs`, `dispatch/planning.rs`, `job/termination.rs`, or convergence `command/context/finalization/system_actions/types.rs` files.

## Decision Log

- Decision: Keep behavior and public function names stable, but prefer the existing `job` facade as the external entry point for job dispatch and job end operations.
  Rationale: The objective is about unfinished module splits and dual entry points, not changing domain behavior or route semantics.
  Date/Author: 2026-05-09 / Codex

- Decision: Preserve unrelated local and untracked work, and layer this refactor on top only where it touches the requested modules.
  Rationale: The worktree is dirty and repository instructions prohibit reverting changes made by the user or other prior work.
  Date/Author: 2026-05-09 / Codex

## Outcomes & Retrospective

Complete. Dispatch planning now lives inside `dispatch.rs`, non-success job termination lives inside `job_completion.rs`, and the public `job` facade is the job-operation entry point. Convergence production code now lives in `flow.rs` and `service.rs` behind the unchanged `convergence` facade. The usecase tests and dependent crate check pass.

## Context and Orientation

The crate `crates/ingot-usecases` contains application-level Rust workflows. A "dispatch" workflow chooses the next workflow step, creates a queued `Job`, fills job input from authoring history or workspaces, persists the job, and performs side effects such as investigation refs. Today this is split between `src/job_dispatch.rs`, which builds a queued job in memory, and `src/dispatch.rs`, which performs persistence and side effects.

A "job end" workflow is the transition from an active job to a terminal state. Successful completion currently lives in `src/job_completion.rs` as `CompleteJobService`; cancellation, failure, and expiration live in `src/job_lifecycle.rs`. The stable public module for job operations is `src/job.rs`.

A "convergence" workflow prepares and finalizes integration work for an item revision. Its public API is `crate::convergence`, currently implemented by several production files under `src/convergence/`: `command.rs`, `context.rs`, `finalization.rs`, `system_actions.rs`, and `types.rs`, plus test files.

## Plan of Work

First, make dispatch ownership explicit by nesting or moving the pure dispatch functions under the dispatch workflow and re-exporting them through both `crate::dispatch` and `crate::job`. Remove the separate top-level `job_dispatch` module from `src/lib.rs` so there is no second crate-level dispatch implementation entry point.

Second, move non-success job termination under the job completion/job facade surface. Update callers in `ingot-http-api` and `ingot-agent-runtime` to import `ingot_usecases::job` for `cancel_job`, `fail_job`, and `expire_job`. Remove `pub mod job_lifecycle` from `src/lib.rs` so the crate no longer exposes two public modules for ending jobs.

Third, reduce convergence production spread. Merge the current context and finalization helpers with their related command/system-action code into fewer modules, keep `convergence/mod.rs` as the public re-export surface, and leave tests in dedicated test files if that keeps the production files readable.

## Concrete Steps

Run commands from `/Users/aa/Documents/ingot`.

Inspect structure:

    rg -n "job_dispatch|job_lifecycle|crate::convergence|ingot_usecases::convergence" crates apps
    wc -l crates/ingot-usecases/src/dispatch.rs crates/ingot-usecases/src/job_dispatch.rs crates/ingot-usecases/src/job_lifecycle.rs crates/ingot-usecases/src/job_completion.rs crates/ingot-usecases/src/convergence/*.rs

After edits, validate:

    cargo fmt --all
    cargo test -p ingot-usecases
    cargo check -p ingot-http-api -p ingot-agent-runtime --all-targets

## Validation and Acceptance

Acceptance is met when there is no top-level `mod job_dispatch` or public `pub mod job_lifecycle` in `crates/ingot-usecases/src/lib.rs`, callers use the `job` facade for job dispatch and job end operations, and `crate::convergence` still exports its existing public service, traits, DTOs, and helper functions from fewer production implementation files. `cargo test -p ingot-usecases` must pass, and `cargo check -p ingot-http-api -p ingot-agent-runtime --all-targets` must pass to prove downstream callers still compile.

## Idempotence and Recovery

The work is source-only. If moving modules creates unresolved imports, restore the missing `pub use` in `job.rs` or `convergence/mod.rs` rather than changing behavior. If a test fails, prefer the narrowest source-compatible fix and rerun the failing command before widening validation.

## Artifacts and Notes

Initial structure evidence:

    crates/ingot-usecases/src/lib.rs contains `mod job_dispatch;` and `pub mod job_lifecycle;`.
    crates/ingot-usecases/src/convergence/ currently contains production files `command.rs`, `context.rs`, `finalization.rs`, `system_actions.rs`, and `types.rs`.

Observed interim validation:

    cargo check -p ingot-usecases --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.33s

    cargo check -p ingot-usecases --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.57s

Final validation:

    cargo test -p ingot-usecases
    test result: ok. 90 passed; 0 failed

    cargo check -p ingot-http-api -p ingot-agent-runtime --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 16.69s

    cargo clippy -p ingot-usecases -p ingot-http-api -p ingot-agent-runtime --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 14.79s

    cargo fmt --all --check
    # no output

    git diff --check
    # no output

Final structure evidence:

    rg --files crates/ingot-usecases/src | rg 'job_dispatch|job_lifecycle|dispatch/planning|job/termination|convergence/(command|context|finalization|system_actions|types)\\.rs'
    # no output

## Interfaces and Dependencies

At the end of this refactor, `crate::job` must export `CompleteJobCommand`, `CompleteJobError`, `CompleteJobResult`, `CompleteJobService`, `DispatchJobCommand`, `dispatch_job`, `retry_job`, `JobTerminationResult`, `cancel_job`, `fail_job`, and `expire_job`.

At the end of this refactor, `crate::convergence` must continue exporting `SystemActionItemState`, `SystemActionProjectState`, `ConvergenceApprovalContext`, `ApprovalFinalizeReadiness`, `FinalizePreparedTrigger`, `CheckoutFinalizationReadiness`, `FinalizeTargetRefResult`, `RejectApprovalTeardown`, `RejectApprovalContext`, `ConvergenceCommandPort`, `ConvergenceSystemActionPort`, `PreparedConvergenceFinalizePort`, `ConvergenceQueuePrepareContext`, `should_prepare_convergence`, `should_invalidate_prepared_convergence`, `should_auto_finalize_prepared_convergence`, `find_or_create_finalize_operation`, `ConvergenceService`, `build_convergence_queue_entry`, `build_convergence_approval_context`, `build_reject_approval_context`, `finalize_prepared_convergence`, `promote_queue_heads`, and `invalidate_prepared_convergence`.

Revision note: created this ExecPlan because the current objective is a significant internal refactor spanning multiple Rust modules and the repository requires an ExecPlan for significant refactors.
