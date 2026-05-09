# Preserve Source Error Classifications

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds.

This document follows `.agent/PLANS.md`.

## Purpose / Big Picture

Ingot currently loses important error classifications when infrastructure failures cross between HTTP adapters, usecases, runtime code, and git helpers. Repository conflicts, protocol failures, workspace state mismatches, and git command failures can become generic internal strings. After this change, source errors keep their type and source chain until the API boundary decides the public HTTP response. A maintainer can see this working through focused Rust tests that assert typed variants instead of opaque strings.

## Progress

- [x] (2026-04-30T15:38:41Z) Inspected current error flows in `crates/ingot-http-api/src/router/infra_ports.rs`, `crates/ingot-agent-runtime/src/runtime_ports.rs`, `crates/ingot-git/src/commands.rs`, and related support modules.
- [x] (2026-04-30T15:38:41Z) Chose the boundary-safe design: `ingot-usecases` stays independent from `ingot-git` and `ingot-workspace`, while source errors are preserved through crate-neutral infrastructure variants.
- [x] (2026-04-30T15:45:56Z) Add structured git command errors.
- [x] (2026-04-30T15:45:56Z) Add source-preserving usecase infrastructure error categories.
- [x] (2026-04-30T15:45:56Z) Remove inward `ApiError` to `UseCaseError` conversions from usecase port adapters.
- [x] (2026-04-30T15:45:56Z) Preserve usecase/runtime conversions without collapsing to strings.
- [x] (2026-04-30T15:45:56Z) Add focused tests for git context, usecase source chains, runtime conversion, and HTTP mapping.
- [x] (2026-04-30T15:45:56Z) Ran `cargo test -p ingot-git -p ingot-usecases -p ingot-http-api -p ingot-agent-runtime`; it passed.
- [x] (2026-04-30T15:47:20Z) Ran `make check`; it passed.
- [x] (2026-04-30T15:47:20Z) Ran `make test`; it passed.

## Surprises & Discoveries

- Observation: `HttpInfraAdapter` exposes helpers returning `ApiError` and then converts those errors back to `UseCaseError` inside port trait implementations.
  Evidence: `.map_err(api_to_uc)` appears throughout `crates/ingot-http-api/src/router/infra_ports.rs`.

- Observation: runtime/usecase conversion only preserves repository errors today.
  Evidence: `usecase_to_runtime_error` maps every other `UseCaseError` to `RuntimeError::InvalidState(other.to_string())`, and `usecase_from_runtime_error` maps every other `RuntimeError` to `UseCaseError::Internal(other.to_string())`.

- Observation: `GitCommandError::CommandFailed(String)` stores only stderr or an ad hoc message.
  Evidence: `crates/ingot-git/src/commands.rs` constructs `CommandFailed(stderr.to_string())` for non-zero git exits.

- Observation: Git stderr content varies by command/version enough that tests should assert stable structured fields rather than exact stderr text.
  Evidence: The first version of the new git command failure test overfit to a `rev-parse` stderr substring and was revised to assert cwd, args, and non-zero exit status.

## Decision Log

- Decision: Preserve crate boundaries by adding crate-neutral infrastructure categories to `UseCaseError`, rather than making `ingot-usecases` depend on `ingot-git` or `ingot-workspace`.
  Rationale: This keeps the architecture boundary described by `AGENTS.md` intact while retaining source chains for debugging.
  Date/Author: 2026-04-30 / Codex

- Decision: Keep HTTP JSON response shaping inside `ApiError::into_response`.
  Rationale: Public API status codes and redacted messages belong at the HTTP boundary, not in usecase ports.
  Date/Author: 2026-04-30 / Codex

## Outcomes & Retrospective

Completed. Error classifications are now preserved through structured git errors, source-preserving usecase infrastructure errors, and runtime/usecase conversion paths. HTTP response mapping remains at the API boundary and redacts git/internal infrastructure details while retaining conflict mappings for workspace state failures.

## Context and Orientation

The important crates are `ingot-git`, `ingot-usecases`, `ingot-agent-runtime`, and `ingot-http-api`. `ingot-git` owns low-level process execution against git repositories. `ingot-usecases` owns application workflows and must not depend on HTTP, database implementation details, or git/workspace infrastructure crates. `ingot-agent-runtime` adapts background runtime work into usecase ports. `ingot-http-api` adapts usecases to Axum route handlers and should be the only layer that maps internal errors to public HTTP status codes and JSON messages.

The current problem has three main forms. First, `GitCommandError::CommandFailed(String)` loses command context and typed failure classification. Second, HTTP adapter helpers return `ApiError` even when implementing usecase ports, then convert `ApiError` back into `UseCaseError`. Third, runtime conversion functions preserve repository errors but collapse other usecase/runtime errors into strings.

## Plan of Work

Start in `crates/ingot-git/src/commands.rs` by changing `GitCommandError` to structured variants. Add helper constructors for command failures and optional verification failures so call sites do not duplicate stdout/stderr/status decoding. Update `crates/ingot-git/src/project_repo.rs` and `crates/ingot-git/src/commit.rs` to use semantic variants for blocked checkout sync, invalid mirror path, and unexpected fetched OID.

Next, extend `crates/ingot-usecases/src/error.rs` with a crate-neutral `UseCaseInfraError` and a `UseCaseError::Infrastructure` variant. The infrastructure error should preserve boxed source errors where possible and use stable categories such as `Git`, `WorkspaceBusy`, `WorkspaceStateMismatch`, `WorkspaceInvalidState`, `Io`, `Serialization`, and `External`.

Then update `crates/ingot-http-api/src/router/support/errors.rs` so git and workspace conversion helpers return `UseCaseError` for usecase-port paths and `ApiError` only for route-boundary paths. Update `infra_ports.rs`, `convergence_port.rs`, and item projection call sites so inward conversions no longer pass through `ApiError`.

Finally, update `crates/ingot-agent-runtime/src/runtime_ports.rs` and `crates/ingot-agent-runtime/src/lib.rs` so runtime/usecase conversions preserve typed usecase errors and infrastructure categories. Runtime-only invariant failures can remain runtime invariant errors.

## Concrete Steps

Run commands from `/Users/aa/Documents/ingot`.

After edits, run:

    cargo test -p ingot-git -p ingot-usecases -p ingot-http-api -p ingot-agent-runtime
    make check
    make test

## Validation and Acceptance

Acceptance is met when tests demonstrate that git failures retain command context, usecase infrastructure failures retain source chains and classifications, HTTP responses still expose stable redacted JSON messages, and runtime/usecase conversion no longer maps all non-repository errors to generic strings.

## Idempotence and Recovery

The changes are source edits only. Re-running tests is safe. If a migration step fails, inspect the compiler error, update call sites to use the new typed constructors, and rerun the narrow crate tests before the workspace checks.

## Artifacts and Notes

Initial evidence:

    crates/ingot-http-api/src/router/infra_ports.rs: .map_err(api_to_uc)
    crates/ingot-agent-runtime/src/runtime_ports.rs: other => RuntimeError::InvalidState(other.to_string())
    crates/ingot-git/src/commands.rs: CommandFailed(String)

## Interfaces and Dependencies

`ingot-usecases` must not gain dependencies on `ingot-git` or `ingot-workspace`. Source preservation uses `Box<dyn std::error::Error + Send + Sync>` in crate-neutral variants. Public HTTP JSON remains shaped as:

    { "error": { "code": "...", "message": "..." } }
