# Tasks 4-6 mechanical refactors

This ExecPlan is a living document. The sections `Progress`, `Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must be kept up to date as work proceeds. This document follows `.agent/PLANS.md`.

## Purpose / Big Picture

These tasks reduce internal coupling without changing runtime behavior. The HTTP router should import the concrete types each route file uses instead of relying on a wildcard `deps` prelude. The agent adapters should share the small parsing helpers Codex and Claude already duplicate. The HTTP API and agent runtime tests should use `ingot-test-support` for reusable database, router, and git setup where public crate boundaries allow it. Success is observable by the requested crate-level checks and tests passing with the same external behavior.

## Progress

- [x] (2026-04-28T19:27:17Z) Read repository instructions, relevant skill guidance, and current worktree state.
- [x] (2026-04-28T19:38:00Z) Replaced router `use super::deps::*` imports with explicit route-local imports and removed the now-unused `deps` module.
- [x] (2026-04-28T19:33:00Z) Extracted shared agent output segment helpers for raw fallback segments, lifecycle segments, and message text extraction; `cargo test -p ingot-agent-adapters` passes.
- [x] (2026-04-28T19:36:00Z) Moved HTTP route fixture builder, seeded database, and job insertion helpers into `ingot-test-support::http`; replaced broad `unused_imports` allowances with targeted allowances on per-binary re-exports.
- [x] (2026-04-28T19:38:07Z) Ran final `cargo check -p ingot-http-api --all-targets`, `cargo test -p ingot-agent-adapters`, `cargo test -p ingot-http-api`, and `cargo test -p ingot-agent-runtime` after formatting.
- [x] (2026-04-28T20:07:00Z) Re-read the modified Rust code with a simplification pass, removed small avoidable allocations/clones, replaced one library-side JSON `expect` with explicit object construction, and reran validation.

## Surprises & Discoveries

- Observation: Several Task 4 target files and `ingot-usecases/src/dispatch.rs` already contain local modifications before this plan starts.
  Evidence: `git status --short` shows modified router files and `crates/ingot-usecases/src/dispatch.rs`.
- Observation: Removing the router prelude exposed a test module dependency on parent imports for `GitRef` and `CommitOid`.
  Evidence: `cargo check -p ingot-http-api --all-targets` failed until those imports were added directly inside `router/dispatch.rs` tests.
- Observation: Codex and Claude share text extraction mechanics, but Claude recursively reads object `content` while Codex does not.
  Evidence: The new helper accepts provider-specific extraction config, preserving the previous parser tests for both adapters.
- Observation: The runtime shared harness is included from both integration tests and in-crate unit tests, where the runtime crate is named differently.
  Evidence: Replacing the integration-test `include!` bridge with a plain module made `cargo test -p ingot-agent-runtime` fail in `src/tests.rs`; the helper remains local because moving it to `ingot-test-support` would require a dependency on the runtime crate itself.
- Observation: The simplification pass found only small local improvements rather than structural issues.
  Evidence: The follow-up edits were limited to `ingot-workspace` path/state ownership cleanup, `ingot-test-support::http` deriving debug/equality and moving an owned JSON payload, `ingot-usecases::dispatch` payload construction, and a test `expect`.

## Decision Log

- Decision: Preserve existing local modifications and layer Task 4-6 edits on top.
  Rationale: The worktree is dirty in files relevant to the request, and repository instructions prohibit reverting user or prior generated work.
  Date/Author: 2026-04-28 / Codex
- Decision: Move HTTP fixture plumbing to `ingot-test-support::http`, but keep runtime `TestHarness` and fake runners local.
  Rationale: HTTP fixture helpers only depend on public domain/store APIs, while runtime harness helpers depend on `ingot-agent-runtime` internals and traits.
  Date/Author: 2026-04-28 / Codex
- Decision: Keep the refinement pass scoped to behavior-neutral ownership/import/payload cleanups rather than broader route DTO wildcard cleanup or dispatch API reshaping.
  Rationale: The request was to preserve exact functionality in recently modified code; wider signature or import churn would add review noise without improving correctness.
  Date/Author: 2026-04-28 / Codex

## Outcomes & Retrospective

The router wildcard prelude is gone, and route modules now declare the imports they use. Codex and Claude adapter parsing share the overlapping segment helpers while keeping provider-specific extraction rules. HTTP route fixture plumbing that depends only on public crates now lives in `ingot-test-support::http`; runtime helpers that need `ingot-agent-runtime` internals remain local. A follow-up simplification pass tightened a few ownership and JSON-construction details without changing behavior.

## Context and Orientation

`crates/ingot-http-api/src/router` contains Axum route modules. A route prelude means one module re-exports many names so other route files can import everything with `use super::deps::*`; Task 4 removes that pattern and makes each route file import only what it uses.

`crates/ingot-agent-adapters/src/codex.rs` and `crates/ingot-agent-adapters/src/claude_code.rs` parse provider output. Both providers have raw fallback output, lifecycle notifications, and text extraction from message-like events. Task 5 moves only overlapping mechanics into shared helpers while preserving provider-specific matching.

`crates/ingot-http-api/tests/common` and `crates/ingot-agent-runtime/tests/common` contain integration-test setup. `crates/ingot-test-support` is the shared public test helper crate. Task 6 moves reusable setup there when it does not need private internals from the consumer crate.

## Plan of Work

First, enumerate every router file that imports `super::deps::*`, read its actual symbol usage, and replace the wildcard with explicit imports from Axum, domain crates, usecase crates, and local router modules. After all route files no longer use `deps`, delete `deps.rs` and remove `mod deps` if it has no remaining purpose.

Second, add a small internal helper module in `crates/ingot-agent-adapters/src` for shared segment operations. Update Codex and Claude parsing to call those helpers only at points where the existing behavior is already identical. Add focused unit tests for the helper behavior if those cases are not already covered.

Third, inspect integration test `common` modules for duplicated temp git repo, migrated database, and router setup. Extend `ingot-test-support` with helpers that depend only on public crates, then update tests to use them. Keep helpers that need private runtime internals local.

## Concrete Steps

Run all commands from `/Users/aa/Documents/ingot`. Use `rg` to find wildcard imports and duplicated helper names. Use `cargo fmt` after edits. Validate with the commands listed in Progress and Validation.

## Validation and Acceptance

Task 4 is accepted when `rg "use super::deps::\\*" crates/ingot-http-api/src/router` finds nothing and `cargo check -p ingot-http-api --all-targets` passes. Task 5 is accepted when `cargo test -p ingot-agent-adapters` passes. Task 6 is accepted when `cargo test -p ingot-http-api` and `cargo test -p ingot-agent-runtime` pass.

## Idempotence and Recovery

Edits are source-only and can be retried safely. If a validation command fails, inspect the compiler or test output, make the smallest targeted fix, and rerun the narrow command before widening validation.

## Artifacts and Notes

- `cargo check -p ingot-http-api --all-targets` passed.
- `cargo test -p ingot-agent-adapters` passed.
- `cargo test -p ingot-http-api` passed.
- `cargo test -p ingot-agent-runtime` passed.
- Follow-up validation passed: `cargo check --workspace --all-targets`, `cargo clippy -p ingot-usecases -p ingot-workspace -p ingot-test-support -p ingot-agent-adapters --all-targets -- -D warnings`, `cargo clippy -p ingot-http-api -p ingot-agent-runtime --all-targets -- -D warnings`, `cargo test -p ingot-workspace`, `cargo test -p ingot-agent-adapters`, `cargo test -p ingot-http-api`, `cargo test -p ingot-agent-runtime`, `cargo fmt --check`, and `git diff --check`.

## Interfaces and Dependencies

The shared adapter helper module should remain crate-private unless a public API need appears. Any new `ingot-test-support` helpers must be public only when consumed by other crates' tests and must avoid depending on private modules from those crates.
