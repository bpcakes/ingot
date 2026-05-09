# Extract HTTP Route Orchestration Into Usecases

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. This document follows `.agent/PLANS.md`.

## Purpose / Big Picture

The HTTP API currently does more than translate requests and responses: several route handlers mutate domain objects, decide persistence order, append activities, tear down active lane state, refresh revision context, and launch follow-up dispatch. After this change, those application workflows live in `ingot-usecases`, where they can be tested and reused without Axum. A user can see the same HTTP behavior as before by running the item, finding, and job route tests, but the boundary is cleaner: routes parse input, call usecases, and map output.

## Progress

- [x] (2026-04-30) Investigated current item, finding, and job route orchestration and existing repository/infra ports.
- [x] (2026-04-30) Add shared application infra and revision-context helpers to `ingot-usecases`.
- [x] (2026-04-30) Extract item command workflows from `crates/ingot-http-api/src/router/items/mod.rs`.
- [x] (2026-04-30) Extract finding triage and promotion workflows from `crates/ingot-http-api/src/router/findings.rs`.
- [x] (2026-04-30) Extract job completion follow-up workflow from `crates/ingot-http-api/src/router/jobs.rs`.
- [x] (2026-04-30) Run focused Rust checks and update this plan with outcomes.
- [x] (2026-05-09 18:29Z) Reopened the plan for the remaining convergence HTTP boundary hotspot in `crates/ingot-http-api/src/router/convergence_port.rs` and `crates/ingot-http-api/src/router/convergence_route_adapter.rs`.
- [x] (2026-05-09) Move pure convergence predicate and state-context loading logic from HTTP into `ingot-usecases`.
- [x] (2026-05-09) Refactor convergence routes to call `ConvergenceService` directly and delete the extra route-adapter wrapper.
- [x] (2026-05-09) Run focused convergence route/usecase tests and Rust checks.

## Surprises & Discoveries

- Observation: `ingot_usecases::teardown::teardown_revision_lane` already owns the database mutation portion of lane teardown, but HTTP still owns post-teardown revision-context refresh and workspace file removal.
  Evidence: `crates/ingot-http-api/src/router/app.rs::teardown_revision_lane_state`.
- Observation: dispatch provisioning already has an infra trait in `ingot_usecases::dispatch::DispatchInfraPort`, so the new application infra should complement it rather than replace it.
  Evidence: `crates/ingot-http-api/src/router/infra_ports.rs` implements `DispatchInfraPort` for `HttpInfraAdapter`.
- Observation: Moving item creation exposed that HTTP had been normalizing target refs before constructing the initial revision.
  Evidence: `tests/item_routes.rs` expected the persisted revision target ref to match the normalized branch ref, so the usecase now parses requested target refs with `GitRef::parse_target_ref`.
- Observation: The HTTP adapter's generic API-to-usecase error conversion was turning `InvalidTargetRef` into an internal error.
  Evidence: invalid branch route coverage failed until `HttpInfraAdapter::api_to_uc` delegated to `support::errors::api_to_usecase_error`.
- Observation: Fresh review found that the extracted job completion workflow had lost the project mutation lock around best-effort follow-up dispatch.
  Evidence: old `auto_dispatch_projected_review_job` acquired `ProjectLocks`; `complete_job_workflow` now takes `project_locks` and reacquires the lock before dispatch.
- Observation: Fresh review found top-level project/item/finding `NotFound` repository errors in extracted usecases could map to HTTP 500 through `UseCaseError::Repository`.
  Evidence: command modules now map those boundary loads to `ProjectNotFound`, `ItemNotFound`, or `FindingNotFound` before returning to HTTP.
- Observation: `crates/ingot-http-api/src/router/convergence_port.rs` still contains pure convergence predicates and state selection helpers even after the earlier extraction.
  Evidence: `approval_finalize_readiness`, `has_active_job_for_revision`, `has_active_convergence_for_revision`, and `prepared_convergence_for_revision` operate only on domain values and return `ingot-usecases::convergence` types.
- Observation: `crates/ingot-http-api/src/router/convergence_route_adapter.rs` wraps `ConvergenceService<HttpConvergencePort>` with three one-line methods that only map `UseCaseError` into `ApiError`.
  Evidence: each method constructs `self.service()` and calls `queue_prepare`, `approve_item`, or `reject_item_approval`.

## Decision Log

- Decision: Add orchestration services in `ingot-usecases` and keep presentation projections in HTTP.
  Rationale: Item detail responses depend on HTTP response DTOs and projection helpers, while mutation sequencing is application logic.
  Date/Author: 2026-04-30 / Codex.
- Decision: Preserve best-effort auto-dispatch behavior by returning dispatch errors as non-fatal outcomes where routes currently log and continue.
  Rationale: The refactor must not change HTTP behavior.
  Date/Author: 2026-04-30 / Codex.
- Decision: Move reusable convergence context construction into `ingot-usecases` rather than into another HTTP helper module.
  Rationale: approval readiness and active-state selection are application workflow decisions and can be tested without Axum or route DTOs.
  Date/Author: 2026-05-09 / Codex.
- Decision: Delete `HttpConvergenceRouteAdapter` if routes can construct and call `ConvergenceService<HttpConvergencePort>` directly.
  Rationale: the adapter adds a wrapper around a wrapper and does not own domain logic or transport-specific response construction.
  Date/Author: 2026-05-09 / Codex.

## Outcomes & Retrospective

Item, finding, and job completion orchestration now lives in `ingot-usecases`, while HTTP route handlers construct command inputs, invoke usecases, log non-fatal best-effort dispatch failures, and map outputs back into existing response DTOs.

The 2026-05-09 convergence boundary pass moved approval/reject context construction and pure predicate helpers into `crates/ingot-usecases/src/convergence/context.rs`. `HttpConvergencePort` now loads persisted state and delegates context decisions to `ingot-usecases`, while `crates/ingot-http-api/src/router/convergence.rs` constructs `ConvergenceService<HttpConvergencePort>` directly. `crates/ingot-http-api/src/router/convergence_route_adapter.rs` was deleted because it only wrapped the service and mapped `UseCaseError` to `ApiError`.

## Context and Orientation

`crates/ingot-http-api/src/router/items/mod.rs` currently owns item metadata updates, revision creation, lifecycle commands, activity emission, and lane teardown. `crates/ingot-http-api/src/router/findings.rs` owns finding triage/promotion persistence, approval transitions, investigation ref cleanup, and follow-up dispatch. `crates/ingot-http-api/src/router/jobs.rs` owns completion follow-up activity, revision context refresh, and projected review dispatch after `CompleteJobService`.

`ingot-usecases` already contains pure constructors and workflow functions such as `item::create_manual_item`, `finding::triage_finding`, `dispatch::auto_dispatch_review`, `teardown::teardown_revision_lane`, and `CompleteJobService`. The remediation moves application sequencing into new usecase modules while keeping Axum request parsing and response DTOs in HTTP.

## Plan of Work

Add an application infra trait in `ingot-usecases` for target-ref validation, ref resolution, commit reachability, mirror refresh, changed path calculation, and workspace file cleanup. Implement it for `HttpInfraAdapter`.

Add shared usecase helpers for revision-context refresh and revision-lane teardown with side effects. Use existing repository traits from `ingot-domain::ports` so `ingot_store_sqlite::Database` can satisfy the services.

Add item command, finding command, and job completion workflow modules. Refactor the HTTP handlers to construct command inputs from existing request DTOs, invoke the relevant usecase, then map outputs into the existing response DTOs.

For the convergence follow-up, add helper functions in `crates/ingot-usecases/src/convergence` that build approval and reject-approval contexts from loaded domain values. Move the pure predicates currently in `crates/ingot-http-api/src/router/convergence_port.rs` into those helpers. Then update `HttpConvergencePort` so it loads projects, items, revisions, jobs, convergences, queue entries, and resolved target refs, delegates domain decisions to `ingot-usecases`, and returns the existing context structs. Finally, remove `crates/ingot-http-api/src/router/convergence_route_adapter.rs` and have `crates/ingot-http-api/src/router/convergence.rs` construct `ConvergenceService::new(HttpConvergencePort::new(&state))` directly.

## Validation and Acceptance

Run from `/Users/aa/Documents/ingot`:

    cargo test -p ingot-usecases
    cargo test -p ingot-http-api --test item_routes
    cargo test -p ingot-http-api --test finding_routes
    cargo test -p ingot-http-api --test job_routes
    cargo test -p ingot-http-api --test convergence_routes
    make check
    make test

Acceptance is that the focused route tests keep passing with unchanged HTTP behavior and `ingot-http-api` no longer directly owns the cited mutation sequencing.

For the convergence follow-up, acceptance additionally requires `crates/ingot-http-api/src/router/convergence_route_adapter.rs` to be absent, `crates/ingot-http-api/src/router/convergence_port.rs` to contain no domain-only predicate helpers such as `approval_finalize_readiness`, and the moved helpers to be covered by `ingot-usecases` tests or by focused `ingot-http-api` convergence route tests that exercise the same behavior.

## Idempotence and Recovery

All edits are source-only. If a test fails, rerun the focused command after fixing the specific module. Do not reset unrelated dirty files in the working tree.

## Artifacts and Notes

- `cargo check -p ingot-usecases`: passed.
- `cargo check -p ingot-http-api`: passed.
- `cargo test -p ingot-usecases`: passed, 84 unit tests plus doc tests.
- `cargo test -p ingot-http-api --test item_routes`: passed, 15 tests.
- `cargo test -p ingot-http-api --test finding_routes`: passed, 6 tests.
- `cargo test -p ingot-http-api --test job_routes`: passed, 14 tests.
- `make check`: passed.
- `make test`: passed.
- `cargo fmt -p ingot-usecases -p ingot-http-api -- --check`: passed.
- Fresh-review validation after lock and not-found mapping fixes: `cargo check -p ingot-usecases -p ingot-http-api`, `cargo test -p ingot-http-api --test job_routes`, `cargo test -p ingot-http-api --test item_routes`, `cargo test -p ingot-http-api --test finding_routes`, `cargo test -p ingot-usecases`, and `git diff --check` all passed.
- Convergence follow-up validation: `cargo fmt -p ingot-usecases -p ingot-http-api -- --check`, `cargo check -p ingot-usecases -p ingot-http-api`, `cargo test -p ingot-usecases convergence::context`, `cargo test -p ingot-usecases convergence`, `cargo test -p ingot-http-api --test convergence_routes`, `cargo test -p ingot-http-api convergence_port`, and `git diff --check` all passed.
- `rg -n "approval_finalize_readiness|has_active_job_for_revision|prepared_convergence_for_revision|HttpConvergenceRouteAdapter|convergence_route_adapter" crates/ingot-http-api crates/ingot-usecases` now finds the predicate helpers only in `crates/ingot-usecases/src/convergence/context.rs` and no route-adapter references.

## Interfaces and Dependencies

New modules should be exported from `crates/ingot-usecases/src/lib.rs` so HTTP can use them. New command structs should use domain types only, not Axum or HTTP DTOs.
