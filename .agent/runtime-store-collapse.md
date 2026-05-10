# Collapse repository ports into focused store capabilities

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. It follows `.agent/PLANS.md`.

## Purpose / Big Picture

Ingot currently exposes persistence through many per-entity repository traits such as `JobRepository`, `ItemRepository`, and `FindingRepository`. There is one concrete persistence backend, `ingot_store_sqlite::Database`, and each repository trait is implemented by that same type. This makes usecase functions carry long generic bounds even though callers pass one runtime store. After this change, the main command and workflow orchestration entry points depend on focused store capability traits instead of one all-encompassing runtime bundle. A reader can see the change working by searching for `RuntimeStore` and by running the Makefile gates.

## Progress

- [x] (2026-05-09 00:00Z) Inspected repository trait definitions, SQLite implementations, and the major usecase call sites.
- [x] (2026-05-09 00:00Z) Chose a single `RuntimeStore` trait over adding an `ingot-store-sqlite` dependency to `ingot-usecases`.
- [x] (2026-05-09 00:00Z) Replaced the too-wide `RuntimeStore` supertrait with focused capability traits such as `ItemCommandStore`, `FindingCommandStore`, `DispatchStore`, `RevisionLaneTeardownStore`, and `ProjectedReviewDispatchStore`.
- [x] (2026-05-09 00:00Z) Updated the main item, finding, application, job workflow, convergence flow, auto-triage, workspace, dispatch, teardown, and convergence system-action usecase functions to use the focused store boundaries.
- [x] (2026-05-09 00:00Z) Narrowed activity-only helpers in item, finding, and job workflow modules back to `ActivityRepository`.
- [x] (2026-05-09 00:00Z) Ran `cargo fmt`, `cargo check -q`, `cargo test -q --no-run`, `make check`, `make test`, and `make lint`; all completed successfully after the focused capability split.
- [x] (2026-05-10 00:00Z) Moved the usecase-shaped aggregate store traits from `ingot-domain` into `ingot-usecases`.
- [x] (2026-05-10 00:00Z) Split `ItemCommandStore` into operation-specific traits including `CreateItemStore`, `UpdateItemStore`, `ItemRevisionMutationStore`, `ResumeItemStore`, `ReopenItemStore`, and `ProjectedReviewDispatchStore`.
- [x] (2026-05-10 00:00Z) Split `FindingCommandStore` into `ApplyFindingTriageStore`, `PromoteFindingStore`, and `BatchPromoteFindingsStore` so batch promotion no longer requires workspace, git-operation, dispatch, or cleanup capabilities.
- [x] (2026-05-10 00:00Z) Ran `cargo check -q` after moving and splitting the capability traits; it completed successfully.

## Surprises & Discoveries

- Observation: Many SQLite modules already expose inherent `Database` methods with concrete names such as `get_job`, `list_jobs_by_item`, and `create_item_with_revision`.
  Evidence: `crates/ingot-store-sqlite/src/store/job/repository.rs` is mostly a shim from `JobRepository` to these inherent methods.

- Observation: A first attempt to remove the per-entity traits directly created too much call-site churn because many existing methods have duplicate names such as `get`, `update`, and `list_by_project`.
  Evidence: The safe implementation keeps fully qualified repository calls and changes the usecase bound to `RuntimeStore`, which compiled under `cargo check -q` and `cargo test -q --no-run`.

## Decision Log

- Decision: Keep `ingot-usecases` independent from `ingot-store-sqlite` and introduce a single domain-level `RuntimeStore` trait instead of making usecases accept the concrete `Database`.
  Rationale: `ARCHITECTURE.md` says `ingot-usecases` must not depend on `sqlx` concrete types. A single trait preserves the dependency direction while eliminating the long per-entity bounds.
  Date/Author: 2026-05-09 / Codex.

- Decision: Do not keep a single all-capability `RuntimeStore` trait.
  Rationale: The initial aggregate hid the same wide abstraction and caused unrelated capabilities such as agent and finalization access to be required by item commands. Focused capability traits preserve the dependency direction without making every usecase require every repository.
  Date/Author: 2026-05-09 / Codex.

## Outcomes & Retrospective

The main usecase command and workflow functions now accept focused store capabilities in `crates/ingot-usecases/src/application.rs`, `item_commands.rs`, `finding_commands.rs`, `job_workflows.rs`, `finding/auto_triage.rs`, `convergence/flow.rs`, `workspace.rs`, `dispatch.rs`, `teardown.rs`, and `convergence/service.rs`. The old per-entity repository traits and SQLite shim impls still exist, so this is a usecase-boundary simplification rather than a full deletion of the old repository abstraction.

The follow-up review found that `ItemCommandStore` and `FindingCommandStore` were still too broad and that the new aggregate traits belonged in `ingot-usecases`, not the public `ingot-domain::ports` API. The current shape keeps the atomic persistence ports in `ingot-domain`, moves the aggregate capability traits to `crates/ingot-usecases/src/store.rs`, and narrows item and finding command entry points by operation.

## Context and Orientation

`ingot-domain` contains pure data types and ports. The current repository traits live in `crates/ingot-domain/src/ports/repositories/`, and mutation traits live in `crates/ingot-domain/src/ports/mutations.rs`. `ingot-store-sqlite::Database` implements every repository trait through small adapter impls under `crates/ingot-store-sqlite/src/store/`. `ingot-usecases` imports those traits and uses fully-qualified calls such as `<R as JobRepository>::get(repo, id)` to disambiguate duplicate method names like `get` and `update`.

The replacement boundaries introduced in this step are focused store traits in `ingot-usecases`. Some usecase code still uses fully qualified calls such as `<R as JobRepository>::get(repo, id)` where duplicate method names require disambiguation, but the orchestration bounds no longer depend on one full repository bundle.

## Plan of Work

First, create `crates/ingot-usecases/src/store.rs` with focused capability traits. Leave atomic persistence ports and non-store ports such as `ProjectMutationLockPort` and `JobCompletionGitPort` in `ingot-domain` because they represent cross-crate contracts.

Then, update high-level usecase orchestration functions where a single runtime store is already passed as `repo: &R`. Change their bounds from many per-entity repository traits or the old aggregate to the narrow capability trait for that workflow, retaining fully qualified calls where duplicate method names require disambiguation.

Finally, run `cargo fmt`, `cargo check -q`, `cargo test -q --no-run`, and `make check`.

## Concrete Steps

From `/Users/aa/Documents/ingot`, run:

    cargo fmt
    cargo check -q
    cargo test -q --no-run
    make check
    make test

`make check` should complete successfully for the Rust workspace.

## Validation and Acceptance

Acceptance requires all of the following:

Usecase command and workflow functions in `application.rs`, `item_commands.rs`, `finding_commands.rs`, `job_workflows.rs`, `finding/auto_triage.rs`, `convergence/flow.rs`, `workspace.rs`, `dispatch.rs`, `teardown.rs`, and `convergence/service.rs` no longer depend on a full all-capability runtime-store trait.

Running `rg "RuntimeStore" crates -n` should produce no matches.

Running `rg "ItemCommandStore|FindingCommandStore" crates -n` should produce no matches.

Running `rg "pub use store|mod store" crates/ingot-domain/src/ports crates/ingot-usecases/src/lib.rs -n` should show `mod store` only in `ingot-usecases`, not in `ingot-domain::ports`.

Running the broad usecase repository-bound search may still show small private helper bounds such as `A: ActivityRepository` and the existing atomic `JobCompletionRepository` port. These are intentionally narrow and are not the all-capability store smell this plan removes.

Running `make check` should pass.

Running `make test` should pass.

## Idempotence and Recovery

All edits are source-only and can be repeated. If a replacement causes compile errors, use `git diff` to inspect the local patch and fix the specific call site; do not reset unrelated untracked files.

## Artifacts and Notes

The initial search showed repository traits in `crates/ingot-domain/src/ports/repositories/`, shims like `crates/ingot-store-sqlite/src/store/job/repository.rs`, and long bounds in `crates/ingot-usecases/src/item_commands.rs`, `application.rs`, `finding_commands.rs`, `dispatch.rs`, `job_workflows.rs`, `workspace.rs`, `teardown.rs`, and `convergence/*`.

## Interfaces and Dependencies

Focused store capabilities live in `crates/ingot-usecases/src/store.rs`, including `CreateItemStore`, `UpdateItemStore`, `ItemRevisionMutationStore`, `ApplyFindingTriageStore`, `BatchPromoteFindingsStore`, `ProjectedReviewDispatchStore`, `DispatchStore`, `RevisionLaneTeardownStore`, `ConvergenceQueuePromotionStore`, `PreparedConvergenceInvalidationStore`, and `FinalizeOperationStore`.

The structs `StartJobExecutionParams`, `FinishJobNonSuccessParams`, `JobCompletionMutation`, `RevisionLaneTeardownMutation`, `InvalidatePreparedConvergenceMutation`, and `FinalizationMutation` remain in `ingot_domain::ports` because they are persistence command payloads used across crates.
